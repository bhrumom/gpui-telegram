#import <Cocoa/Cocoa.h>
#import <CoreGraphics/CoreGraphics.h>
#import <QuartzCore/QuartzCore.h>
#include <math.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>

static const CGFloat kHudWidth = 316.0;
static const CGFloat kHudHeight = 114.0;
static NSString * const kSystemSettingsBundleID = @"com.apple.systempreferences";

static _Atomic uint64_t g_drag_event_seq = 0;
static _Atomic int32_t g_last_dragged_kind = -1;
static _Atomic uint64_t g_user_closed_seq = 0;
static _Atomic bool g_hud_visible = false;
static _Atomic int32_t g_hud_kind = -1;
static _Atomic uint64_t g_hud_command_gen = 0;

typedef struct {
    BOOL running;
    BOOL hidden;
    BOOL frontmost;
    BOOL hasOnScreenWindow;
    BOOL hasAnyWindow;
    BOOL coveredByOtherApp;
    pid_t pid;
    NSRect frame;     // AppKit bottom-left screen coordinates
    CGRect cgFrame;   // CoreGraphics top-left screen coordinates
} FabushiSettingsWindowState;

static CGFloat fabushi_primary_screen_height(void) {
    NSScreen *primaryScreen = NSScreen.screens.firstObject ?: [NSScreen mainScreen];
    if (primaryScreen && primaryScreen.frame.size.height > 0.0) {
        return primaryScreen.frame.size.height;
    }
    return 900.0;
}

static NSRect fabushi_cg_to_cocoa_rect(CGRect cgRect) {
    CGFloat primaryHeight = fabushi_primary_screen_height();
    CGFloat cocoaY = primaryHeight - cgRect.origin.y - cgRect.size.height;
    return NSMakeRect(cgRect.origin.x, cocoaY, cgRect.size.width, cgRect.size.height);
}

static CGRect fabushi_cocoa_to_cg_rect(NSRect cocoaRect) {
    CGFloat primaryHeight = fabushi_primary_screen_height();
    CGFloat cgY = primaryHeight - cocoaRect.origin.y - cocoaRect.size.height;
    return CGRectMake(cocoaRect.origin.x, cgY, cocoaRect.size.width, cocoaRect.size.height);
}

static NSPoint fabushi_compute_hud_origin(NSRect settingsFrame, BOOL hasSettingsFrame) {
    NSScreen *targetScreen = [NSScreen mainScreen] ?: NSScreen.screens.firstObject;
    if (hasSettingsFrame) {
        NSPoint settingsCenter = NSMakePoint(NSMidX(settingsFrame), NSMidY(settingsFrame));
        BOOL matched = NO;
        for (NSScreen *scr in NSScreen.screens) {
            if (NSPointInRect(settingsCenter, scr.frame)) {
                targetScreen = scr;
                matched = YES;
                break;
            }
        }
        if (!matched) {
            CGFloat bestOverlap = 0.0;
            for (NSScreen *scr in NSScreen.screens) {
                NSRect inter = NSIntersectionRect(settingsFrame, scr.frame);
                CGFloat area = inter.size.width * inter.size.height;
                if (area > bestOverlap) {
                    bestOverlap = area;
                    targetScreen = scr;
                }
            }
        }
    }

    NSRect visible = targetScreen ? targetScreen.visibleFrame : NSMakeRect(0, 0, 1440, 900);
    CGFloat x = 0.0;
    CGFloat y = 0.0;

    if (hasSettingsFrame) {
        // Anchor near the bottom of the System Settings detail pane (matching Gemini HUD placement)
        CGFloat detailCenterX = settingsFrame.origin.x + settingsFrame.size.width * 0.64;
        x = detailCenterX - kHudWidth * 0.5;
        y = settingsFrame.origin.y + 22.0;
    } else {
        x = visible.origin.x + 180.0;
        y = visible.origin.y + 40.0;
    }

    CGFloat minX = visible.origin.x + 12.0;
    CGFloat maxX = NSMaxX(visible) - kHudWidth - 12.0;
    CGFloat minY = visible.origin.y + 12.0;
    CGFloat maxY = NSMaxY(visible) - kHudHeight - 12.0;

    if (x < minX) x = minX;
    if (x > maxX) x = maxX;
    if (y < minY) y = minY;
    if (y > maxY) y = maxY;

    return NSMakePoint(floor(x), floor(y));
}

static BOOL fabushi_is_system_chrome_window(pid_t pid, NSString *ownerName) {
    if (ownerName.length > 0 &&
        ([ownerName isEqualToString:@"WindowServer"] ||
         [ownerName isEqualToString:@"Dock"] ||
         [ownerName isEqualToString:@"Control Center"] ||
         [ownerName isEqualToString:@"ControlCenter"] ||
         [ownerName isEqualToString:@"SystemUIServer"] ||
         [ownerName isEqualToString:@"Notification Center"] ||
         [ownerName isEqualToString:@"NotificationCenter"])) {
        return YES;
    }
    if (pid > 0) {
        NSRunningApplication *app = [NSRunningApplication runningApplicationWithProcessIdentifier:pid];
        NSString *bundleID = app.bundleIdentifier;
        if (bundleID.length > 0 &&
            ([bundleID isEqualToString:@"com.apple.dock"] ||
             [bundleID isEqualToString:@"com.apple.controlcenter"] ||
             [bundleID isEqualToString:@"com.apple.notificationcenterui"] ||
             [bundleID isEqualToString:@"com.apple.systemuiserver"] ||
             [bundleID isEqualToString:@"com.apple.WindowManager"])) {
            return YES;
        }
    }
    return NO;
}

static FabushiSettingsWindowState fabushi_inspect_system_settings(CGWindowID hudWindowNumber) {
    FabushiSettingsWindowState state;
    memset(&state, 0, sizeof(state));

    NSArray<NSRunningApplication *> *apps =
        [NSRunningApplication runningApplicationsWithBundleIdentifier:kSystemSettingsBundleID];
    NSRunningApplication *settingsApp = nil;
    for (NSRunningApplication *app in apps) {
        if (app.isTerminated) {
            continue;
        }
        if (!settingsApp || app.isActive) {
            settingsApp = app;
        }
    }
    if (!settingsApp) {
        return state;
    }

    state.running = YES;
    state.pid = settingsApp.processIdentifier;
    state.hidden = settingsApp.isHidden;

    NSRunningApplication *frontApp = NSWorkspace.sharedWorkspace.frontmostApplication;
    state.frontmost = (frontApp != nil &&
                       !frontApp.isTerminated &&
                       (frontApp.processIdentifier == state.pid ||
                        [frontApp.bundleIdentifier isEqualToString:kSystemSettingsBundleID]));

    CFArrayRef winList = CGWindowListCopyWindowInfo(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID
    );
    if (winList) {
        NSArray *windows = CFBridgingRelease(winList);
        CGRect bestCGRect = CGRectZero;
        CGFloat bestArea = 0.0;
        NSUInteger bestIndex = NSNotFound;

        for (NSUInteger i = 0; i < windows.count; i++) {
            NSDictionary *win = windows[i];
            NSNumber *pidNum = win[(__bridge NSString *)kCGWindowOwnerPID];
            if (!pidNum || pidNum.intValue != state.pid) {
                continue;
            }
            NSNumber *layerNum = win[(__bridge NSString *)kCGWindowLayer];
            if (layerNum && layerNum.intValue != 0) {
                continue;
            }
            NSNumber *alphaNum = win[(__bridge NSString *)kCGWindowAlpha];
            if (alphaNum && alphaNum.doubleValue <= 0.01) {
                continue;
            }
            NSDictionary *boundsDict = win[(__bridge NSString *)kCGWindowBounds];
            if (!boundsDict) {
                continue;
            }
            CGRect cgRect = CGRectZero;
            if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)boundsDict, &cgRect)) {
                continue;
            }
            if (cgRect.size.width < 320.0 || cgRect.size.height < 280.0) {
                continue;
            }
            CGFloat area = cgRect.size.width * cgRect.size.height;
            if (area > bestArea) {
                bestArea = area;
                bestCGRect = cgRect;
                bestIndex = i;
            }
        }

        if (bestArea > 0.0 && bestIndex != NSNotFound) {
            state.hasOnScreenWindow = YES;
            state.hasAnyWindow = YES;
            state.cgFrame = bestCGRect;
            state.frame = fabushi_cg_to_cocoa_rect(bestCGRect);

            NSPoint hudOrigin = fabushi_compute_hud_origin(state.frame, YES);
            CGRect hudCGRect = fabushi_cocoa_to_cg_rect(
                NSMakeRect(hudOrigin.x, hudOrigin.y, kHudWidth, kHudHeight)
            );
            CGFloat hudArea = kHudWidth * kHudHeight;

            // CGWindowListCopyWindowInfo is ordered front-to-back; inspect any window in front of Settings.
            for (NSUInteger i = 0; i < bestIndex; i++) {
                NSDictionary *win = windows[i];
                NSNumber *pidNum = win[(__bridge NSString *)kCGWindowOwnerPID];
                if (!pidNum || pidNum.intValue == state.pid) {
                    continue;
                }
                NSNumber *winNumObj = win[(__bridge NSString *)kCGWindowNumber];
                if (hudWindowNumber != kCGNullWindowID && winNumObj &&
                    (CGWindowID)winNumObj.unsignedIntValue == hudWindowNumber) {
                    continue;
                }
                NSNumber *alphaNum = win[(__bridge NSString *)kCGWindowAlpha];
                double alpha = alphaNum ? alphaNum.doubleValue : 1.0;
                if (alpha <= 0.01) {
                    continue;
                }
                NSNumber *layerNum = win[(__bridge NSString *)kCGWindowLayer];
                int layer = layerNum ? layerNum.intValue : 0;
                if (layer < 0 || layer > 8) {
                    continue;
                }
                NSString *ownerName = win[(__bridge NSString *)kCGWindowOwnerName];
                if (fabushi_is_system_chrome_window((pid_t)pidNum.intValue, ownerName)) {
                    continue;
                }
                NSDictionary *boundsDict = win[(__bridge NSString *)kCGWindowBounds];
                if (!boundsDict) {
                    continue;
                }
                CGRect winRect = CGRectZero;
                if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)boundsDict, &winRect)) {
                    continue;
                }
                if (winRect.size.width < 40.0 || winRect.size.height < 40.0) {
                    continue;
                }

                if (layer == 0) {
                    if (CGRectIntersectsRect(winRect, hudCGRect) ||
                        CGRectIntersectsRect(winRect, bestCGRect)) {
                        state.coveredByOtherApp = YES;
                        break;
                    }
                } else if (alpha >= 0.5 && CGRectIntersectsRect(winRect, hudCGRect)) {
                    CGRect inter = CGRectIntersection(winRect, hudCGRect);
                    CGFloat interArea = inter.size.width * inter.size.height;
                    if (interArea >= hudArea * 0.30) {
                        state.coveredByOtherApp = YES;
                        break;
                    }
                }
            }
        }
    }

    // Distinguish minimized / hidden / other-Space window vs genuinely closed window.
    if (!state.hasOnScreenWindow && !state.hidden) {
        CFArrayRef allList = CGWindowListCopyWindowInfo(
            kCGWindowListOptionAll | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID
        );
        if (allList) {
            NSArray *allWindows = CFBridgingRelease(allList);
            for (NSDictionary *win in allWindows) {
                NSNumber *pidNum = win[(__bridge NSString *)kCGWindowOwnerPID];
                if (!pidNum || pidNum.intValue != state.pid) {
                    continue;
                }
                NSNumber *layerNum = win[(__bridge NSString *)kCGWindowLayer];
                if (layerNum && layerNum.intValue != 0) {
                    continue;
                }
                NSDictionary *boundsDict = win[(__bridge NSString *)kCGWindowBounds];
                if (!boundsDict) {
                    continue;
                }
                CGRect cgRect = CGRectZero;
                if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)boundsDict, &cgRect)) {
                    continue;
                }
                if (cgRect.size.width >= 200.0 && cgRect.size.height >= 160.0) {
                    state.hasAnyWindow = YES;
                    break;
                }
            }
        }
    }

    return state;
}

static void fabushi_perform_with_appearance(NSView *view, void (^block)(void)) {
    if (!block) {
        return;
    }
    if (@available(macOS 11.0, *)) {
        [view.effectiveAppearance performAsCurrentDrawingAppearance:block];
    } else {
        block();
    }
}

@interface FabushiPermissionHudPanel : NSPanel
@end

@implementation FabushiPermissionHudPanel
- (BOOL)canBecomeKeyWindow {
    return NO;
}
- (BOOL)canBecomeMainWindow {
    return NO;
}
@end

@interface FabushiHudBorderOverlayView : NSView
@end

@implementation FabushiHudBorderOverlayView

- (instancetype)initWithFrame:(NSRect)frameRect {
    self = [super initWithFrame:frameRect];
    if (self) {
        self.wantsLayer = YES;
        self.layer.cornerRadius = 16.0;
        self.layer.borderWidth = 1.0;
        [self refreshBorderColor];
    }
    return self;
}

- (void)refreshBorderColor {
    fabushi_perform_with_appearance(self, ^{
        self.layer.borderColor = [[NSColor separatorColor] colorWithAlphaComponent:0.55].CGColor;
    });
}

- (void)viewDidChangeEffectiveAppearance {
    [super viewDidChangeEffectiveAppearance];
    [self refreshBorderColor];
}

- (NSView *)hitTest:(NSPoint)point {
    (void)point;
    return nil;
}

@end

@interface FabushiHudCloseButton : NSButton
@property (nonatomic, assign) BOOL hovered;
@property (nonatomic, strong) NSTrackingArea *hoverTrackingArea;
@end

@implementation FabushiHudCloseButton

- (instancetype)initWithFrame:(NSRect)frameRect {
    self = [super initWithFrame:frameRect];
    if (self) {
        self.wantsLayer = YES;
        self.layer.cornerRadius = frameRect.size.height * 0.5;
        self.layer.masksToBounds = YES;
        [self refreshHoverAppearance];
    }
    return self;
}

- (BOOL)acceptsFirstMouse:(NSEvent *)event {
    (void)event;
    return YES;
}

- (BOOL)shouldDelayWindowOrderingForEvent:(NSEvent *)event {
    (void)event;
    return YES;
}

- (BOOL)mouseDownCanMoveWindow {
    return NO;
}

- (void)refreshHoverAppearance {
    fabushi_perform_with_appearance(self, ^{
        CGFloat alpha = self.hovered ? 0.12 : 0.0;
        self.layer.backgroundColor = [[NSColor labelColor] colorWithAlphaComponent:alpha].CGColor;
    });
}

- (void)viewDidChangeEffectiveAppearance {
    [super viewDidChangeEffectiveAppearance];
    [self refreshHoverAppearance];
}

- (void)updateTrackingAreas {
    [super updateTrackingAreas];
    if (self.hoverTrackingArea) {
        [self removeTrackingArea:self.hoverTrackingArea];
    }
    NSTrackingAreaOptions opts =
        NSTrackingMouseEnteredAndExited | NSTrackingActiveAlways | NSTrackingInVisibleRect;
    self.hoverTrackingArea =
        [[NSTrackingArea alloc] initWithRect:self.bounds options:opts owner:self userInfo:nil];
    [self addTrackingArea:self.hoverTrackingArea];
}

- (void)mouseEntered:(NSEvent *)event {
    (void)event;
    self.hovered = YES;
    [self refreshHoverAppearance];
}

- (void)mouseExited:(NSEvent *)event {
    (void)event;
    self.hovered = NO;
    [self refreshHoverAppearance];
}

@end

@interface FabushiDraggableAppPillView : NSView <NSDraggingSource>
@property (nonatomic, copy) NSString *appPath;
@property (nonatomic, copy) NSString *appName;
@property (nonatomic, copy) NSString *dragHint;
@property (nonatomic, assign) BOOL hovered;
@property (nonatomic, assign) BOOL dragging;
@property (nonatomic, assign) NSPoint mouseDownLocal;
@property (nonatomic, assign) NSPoint mouseDownScreen;
@property (nonatomic, assign) NSRect cachedSettingsFrame;
@property (nonatomic, assign) BOOL cachedSettingsFrameValid;
@property (nonatomic, copy) void (^onDragCompleted)(void);
@property (nonatomic, copy) void (^onDragEndedWithoutDrop)(void);
@property (nonatomic, strong) NSImageView *iconView;
@property (nonatomic, strong) NSTextField *nameLabel;
@property (nonatomic, strong) NSTextField *hintLabel;
@property (nonatomic, strong) NSTrackingArea *trackingArea;
- (void)updateWithAppName:(NSString *)name appPath:(NSString *)path dragHint:(NSString *)hint;
@end

@implementation FabushiDraggableAppPillView

- (instancetype)initWithFrame:(NSRect)frameRect {
    self = [super initWithFrame:frameRect];
    if (self) {
        self.wantsLayer = YES;
        self.layer.cornerRadius = 10.0;
        self.layer.masksToBounds = YES;
        self.layer.borderWidth = 1.0;

        _iconView = [[NSImageView alloc] initWithFrame:NSMakeRect(10.0, 7.0, 24.0, 24.0)];
        _iconView.imageScaling = NSImageScaleProportionallyUpOrDown;
        [self addSubview:_iconView];

        _nameLabel = [NSTextField labelWithString:@"Fabushi"];
        _nameLabel.font = [NSFont systemFontOfSize:13.5 weight:NSFontWeightSemibold];
        _nameLabel.textColor = [NSColor labelColor];
        _nameLabel.lineBreakMode = NSLineBreakByTruncatingTail;
        _nameLabel.frame = NSMakeRect(42.0, 9.0, 152.0, 20.0);
        [self addSubview:_nameLabel];

        _hintLabel = [NSTextField labelWithString:@"按住拖入"];
        _hintLabel.font = [NSFont systemFontOfSize:11.0 weight:NSFontWeightMedium];
        _hintLabel.textColor = [NSColor secondaryLabelColor];
        _hintLabel.alignment = NSTextAlignmentRight;
        _hintLabel.frame = NSMakeRect(198.0, 10.0, frameRect.size.width - 208.0, 18.0);
        [self addSubview:_hintLabel];

        [self refreshAppearance];
    }
    return self;
}

- (void)updateWithAppName:(NSString *)name appPath:(NSString *)path dragHint:(NSString *)hint {
    self.appName = name.length > 0 ? name : @"Fabushi";
    self.appPath = path ?: @"";
    self.dragHint = hint.length > 0 ? hint : @"";

    self.nameLabel.stringValue = self.appName;
    self.hintLabel.stringValue = self.dragHint;

    NSImage *icon = nil;
    if (self.appPath.length > 0 && [[NSFileManager defaultManager] fileExistsAtPath:self.appPath]) {
        icon = [[NSWorkspace sharedWorkspace] iconForFile:self.appPath];
    }
    if (!icon) {
        icon = NSApp.applicationIconImage;
    }
    self.iconView.image = icon;
    [self refreshAppearance];
}

- (void)refreshAppearance {
    fabushi_perform_with_appearance(self, ^{
        CGFloat fillAlpha = self.dragging ? 0.14 : (self.hovered ? 0.10 : 0.055);
        self.layer.backgroundColor = [[NSColor labelColor] colorWithAlphaComponent:fillAlpha].CGColor;
        self.layer.borderColor =
            [[NSColor separatorColor] colorWithAlphaComponent:self.hovered ? 0.65 : 0.35].CGColor;
    });
}

- (void)viewDidChangeEffectiveAppearance {
    [super viewDidChangeEffectiveAppearance];
    [self refreshAppearance];
}

- (void)updateTrackingAreas {
    [super updateTrackingAreas];
    if (self.trackingArea) {
        [self removeTrackingArea:self.trackingArea];
    }
    NSTrackingAreaOptions opts =
        NSTrackingMouseEnteredAndExited | NSTrackingActiveAlways | NSTrackingInVisibleRect;
    self.trackingArea =
        [[NSTrackingArea alloc] initWithRect:self.bounds options:opts owner:self userInfo:nil];
    [self addTrackingArea:self.trackingArea];
}

- (void)resetCursorRects {
    [self addCursorRect:self.bounds cursor:[NSCursor openHandCursor]];
}

- (void)mouseEntered:(NSEvent *)event {
    (void)event;
    self.hovered = YES;
    [self refreshAppearance];
    [[NSCursor openHandCursor] set];
}

- (void)mouseExited:(NSEvent *)event {
    (void)event;
    self.hovered = NO;
    if (!self.dragging) {
        [[NSCursor arrowCursor] set];
    }
    [self refreshAppearance];
}

- (NSView *)hitTest:(NSPoint)point {
    return [super hitTest:point] ? self : nil;
}

- (BOOL)acceptsFirstMouse:(NSEvent *)event {
    (void)event;
    return YES;
}

- (BOOL)shouldDelayWindowOrderingForEvent:(NSEvent *)event {
    (void)event;
    return YES;
}

- (BOOL)mouseDownCanMoveWindow {
    return NO;
}

- (void)mouseDown:(NSEvent *)event {
    self.mouseDownLocal = [self convertPoint:event.locationInWindow fromView:nil];
    self.mouseDownScreen = [NSEvent mouseLocation];
    self.dragging = NO;
    [[NSCursor closedHandCursor] set];
    [self refreshAppearance];
}

- (void)mouseUp:(NSEvent *)event {
    (void)event;
    self.dragging = NO;
    [[NSCursor openHandCursor] set];
    [self refreshAppearance];
}

- (void)mouseDragged:(NSEvent *)event {
    if (self.dragging || self.appPath.length == 0) {
        return;
    }
    NSPoint cur = [self convertPoint:event.locationInWindow fromView:nil];
    CGFloat dx = cur.x - self.mouseDownLocal.x;
    CGFloat dy = cur.y - self.mouseDownLocal.y;
    if (dx * dx + dy * dy < 4.0) {
        return;
    }

    NSURL *fileURL = [NSURL fileURLWithPath:self.appPath];
    if (!fileURL) {
        return;
    }

    self.dragging = YES;
    [self refreshAppearance];
    [[NSCursor closedHandCursor] set];

    NSDraggingItem *item = [[NSDraggingItem alloc] initWithPasteboardWriter:fileURL];
    NSImage *dragIcon = nil;
    if ([[NSFileManager defaultManager] fileExistsAtPath:self.appPath]) {
        dragIcon = [[NSWorkspace sharedWorkspace] iconForFile:self.appPath];
    }
    if (!dragIcon) {
        dragIcon = NSApp.applicationIconImage;
    }
    if (dragIcon) {
        dragIcon = [dragIcon copy];
        dragIcon.size = NSMakeSize(52.0, 52.0);
    }

    NSRect dragFrame = NSMakeRect(cur.x - 26.0, cur.y - 26.0, 52.0, 52.0);
    [item setDraggingFrame:dragFrame contents:dragIcon];
    NSDraggingSession *session =
        [self beginDraggingSessionWithItems:@[item] event:event source:self];
    if (!session) {
        self.dragging = NO;
        [[NSCursor arrowCursor] set];
        [self refreshAppearance];
    }
}

- (NSDragOperation)draggingSession:(NSDraggingSession *)session
sourceOperationMaskForDraggingContext:(NSDraggingContext)context {
    (void)session;
    if (context == NSDraggingContextOutsideApplication) {
        return NSDragOperationCopy | NSDragOperationLink | NSDragOperationGeneric;
    }
    return NSDragOperationCopy;
}

- (void)draggingSession:(NSDraggingSession *)session
           endedAtPoint:(NSPoint)screenPoint
              operation:(NSDragOperation)operation {
    (void)session;
    self.dragging = NO;
    [[NSCursor arrowCursor] set];
    [self refreshAppearance];

    NSRect hudFrame = self.window ? self.window.frame : NSZeroRect;
    BOOL droppedOutsideHud = !self.window || !NSPointInRect(screenPoint, hudFrame);

    CGWindowID hudWinNum =
        (self.window && self.window.windowNumber > 0)
            ? (CGWindowID)self.window.windowNumber
            : kCGNullWindowID;
    FabushiSettingsWindowState st = fabushi_inspect_system_settings(hudWinNum);

    BOOL hasSettingsFrame = st.hasOnScreenWindow || self.cachedSettingsFrameValid;
    NSRect settingsFrame = st.hasOnScreenWindow ? st.frame : self.cachedSettingsFrame;

    BOOL droppedIntoSettings = NO;
    if (operation != NSDragOperationNone) {
        droppedIntoSettings = YES;
    } else if (droppedOutsideHud && hasSettingsFrame &&
               NSPointInRect(screenPoint, NSInsetRect(settingsFrame, -24.0, -24.0))) {
        droppedIntoSettings = YES;
    }

    if (droppedIntoSettings) {
        if (self.onDragCompleted) {
            self.onDragCompleted();
        }
    } else if (self.onDragEndedWithoutDrop) {
        self.onDragEndedWithoutDrop();
    }
}

@end

@interface FabushiPermissionHudController : NSObject
@property (nonatomic, strong) FabushiPermissionHudPanel *panel;
@property (nonatomic, strong) NSVisualEffectView *backdrop;
@property (nonatomic, strong) FabushiHudBorderOverlayView *borderOverlay;
@property (nonatomic, strong) NSTextField *instructionLabel;
@property (nonatomic, strong) FabushiDraggableAppPillView *pillView;
@property (nonatomic, strong) NSTimer *syncTimer;
@property (nonatomic, assign) BOOL sessionActive;
@property (nonatomic, assign) int32_t sessionKind;
@property (nonatomic, assign) BOOL sawSettingsWindow;
@property (nonatomic, assign) NSTimeInterval missingWindowSince;
@property (nonatomic, assign) NSTimeInterval sessionStartedAt;
@property (nonatomic, assign) BOOL workspaceObserversRegistered;
@end

@implementation FabushiPermissionHudController

+ (instancetype)shared {
    static FabushiPermissionHudController *instance = nil;
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        instance = [[FabushiPermissionHudController alloc] init];
        instance.sessionKind = -1;
    });
    return instance;
}

- (void)ensurePanel {
    if (self.panel) {
        return;
    }

    NSRect rect = NSMakeRect(200.0, 200.0, kHudWidth, kHudHeight);
    FabushiPermissionHudPanel *panel = [[FabushiPermissionHudPanel alloc]
        initWithContentRect:rect
                  styleMask:(NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel)
                    backing:NSBackingStoreBuffered
                      defer:NO];
    panel.floatingPanel = YES;
    panel.hidesOnDeactivate = NO;
    panel.level = NSFloatingWindowLevel;
    panel.collectionBehavior =
        NSWindowCollectionBehaviorMoveToActiveSpace |
        NSWindowCollectionBehaviorFullScreenAuxiliary |
        NSWindowCollectionBehaviorTransient |
        NSWindowCollectionBehaviorIgnoresCycle;
    panel.opaque = NO;
    panel.backgroundColor = [NSColor clearColor];
    panel.hasShadow = YES;
    panel.movableByWindowBackground = NO;
    panel.releasedWhenClosed = NO;

    NSVisualEffectView *backdrop =
        [[NSVisualEffectView alloc] initWithFrame:NSMakeRect(0, 0, kHudWidth, kHudHeight)];
    backdrop.material = NSVisualEffectMaterialPopover;
    backdrop.blendingMode = NSVisualEffectBlendingModeBehindWindow;
    backdrop.state = NSVisualEffectStateActive;
    backdrop.wantsLayer = YES;
    backdrop.layer.cornerRadius = 16.0;
    backdrop.layer.masksToBounds = YES;

    FabushiHudBorderOverlayView *borderOverlay =
        [[FabushiHudBorderOverlayView alloc] initWithFrame:backdrop.bounds];
    [backdrop addSubview:borderOverlay];

    // Up arrow icon matching Gemini HUD
    NSView *arrowView = nil;
    if (@available(macOS 11.0, *)) {
        NSImage *sym = [NSImage imageWithSystemSymbolName:@"arrow.up" accessibilityDescription:nil];
        if (sym) {
            NSImageSymbolConfiguration *cfg =
                [NSImageSymbolConfiguration configurationWithPointSize:13.0 weight:NSFontWeightMedium];
            sym = [sym imageWithSymbolConfiguration:cfg];
            NSImageView *iv = [[NSImageView alloc] initWithFrame:NSMakeRect(14.0, 80.0, 16.0, 18.0)];
            iv.image = sym;
            iv.contentTintColor = [NSColor secondaryLabelColor];
            arrowView = iv;
        }
    }
    if (!arrowView) {
        NSTextField *arrowLbl = [NSTextField labelWithString:@"↑"];
        arrowLbl.font = [NSFont systemFontOfSize:13.5 weight:NSFontWeightMedium];
        arrowLbl.textColor = [NSColor secondaryLabelColor];
        arrowLbl.frame = NSMakeRect(14.0, 80.0, 16.0, 18.0);
        arrowView = arrowLbl;
    }
    [backdrop addSubview:arrowView];

    // 2-line instruction label
    NSTextField *instruction = [NSTextField wrappingLabelWithString:@""];
    instruction.font = [NSFont systemFontOfSize:12.5 weight:NSFontWeightMedium];
    instruction.textColor = [NSColor labelColor];
    instruction.maximumNumberOfLines = 2;
    instruction.lineBreakMode = NSLineBreakByWordWrapping;
    instruction.selectable = NO;
    instruction.editable = NO;
    instruction.frame = NSMakeRect(36.0, 58.0, kHudWidth - 72.0, 42.0);
    [backdrop addSubview:instruction];

    // Close button top-right (acceptsFirstMouse so single-click works without activating Fabushi)
    FabushiHudCloseButton *closeBtn =
        [[FabushiHudCloseButton alloc] initWithFrame:NSMakeRect(kHudWidth - 32.0, kHudHeight - 32.0, 22.0, 22.0)];
    closeBtn.bordered = NO;
    closeBtn.bezelStyle = NSBezelStyleRegularSquare;
    closeBtn.target = self;
    closeBtn.action = @selector(handleUserClose:);
    if (@available(macOS 11.0, *)) {
        NSImage *xSym = [NSImage imageWithSystemSymbolName:@"xmark" accessibilityDescription:@"Close"];
        if (xSym) {
            NSImageSymbolConfiguration *cfg =
                [NSImageSymbolConfiguration configurationWithPointSize:10.5 weight:NSFontWeightSemibold];
            closeBtn.image = [xSym imageWithSymbolConfiguration:cfg];
            closeBtn.contentTintColor = [NSColor secondaryLabelColor];
        } else {
            closeBtn.title = @"×";
        }
    } else {
        closeBtn.title = @"×";
    }
    [backdrop addSubview:closeBtn];

    // Draggable app pill at bottom
    FabushiDraggableAppPillView *pill =
        [[FabushiDraggableAppPillView alloc] initWithFrame:NSMakeRect(14.0, 14.0, kHudWidth - 28.0, 38.0)];
    __weak typeof(self) weakSelf = self;
    pill.onDragCompleted = ^{
        [weakSelf handleDragCompleted];
    };
    pill.onDragEndedWithoutDrop = ^{
        [weakSelf syncWithSystemSettings];
    };
    [backdrop addSubview:pill];

    panel.contentView = backdrop;

    self.panel = panel;
    self.backdrop = backdrop;
    self.borderOverlay = borderOverlay;
    self.instructionLabel = instruction;
    self.pillView = pill;
}

- (void)registerWorkspaceObserversIfNeeded {
    if (self.workspaceObserversRegistered) {
        return;
    }
    NSNotificationCenter *wsCenter = NSWorkspace.sharedWorkspace.notificationCenter;
    [wsCenter addObserver:self
                 selector:@selector(handleWorkspaceDidActivateApp:)
                     name:NSWorkspaceDidActivateApplicationNotification
                   object:nil];
    [wsCenter addObserver:self
                 selector:@selector(handleWorkspaceDidDeactivateApp:)
                     name:NSWorkspaceDidDeactivateApplicationNotification
                   object:nil];
    [wsCenter addObserver:self
                 selector:@selector(handleWorkspaceDidHideApp:)
                     name:NSWorkspaceDidHideApplicationNotification
                   object:nil];
    [wsCenter addObserver:self
                 selector:@selector(handleWorkspaceDidUnhideApp:)
                     name:NSWorkspaceDidUnhideApplicationNotification
                   object:nil];
    [wsCenter addObserver:self
                 selector:@selector(handleWorkspaceDidTerminateApp:)
                     name:NSWorkspaceDidTerminateApplicationNotification
                   object:nil];
    [wsCenter addObserver:self
                 selector:@selector(handleWorkspaceActiveSpaceDidChange:)
                     name:NSWorkspaceActiveSpaceDidChangeNotification
                   object:nil];
    self.workspaceObserversRegistered = YES;
}

- (void)runOnMain:(dispatch_block_t)block {
    if (!block) {
        return;
    }
    if ([NSThread isMainThread]) {
        block();
    } else {
        dispatch_async(dispatch_get_main_queue(), block);
    }
}

- (void)handleWorkspaceDidActivateApp:(NSNotification *)notification {
    NSRunningApplication *app = notification.userInfo[NSWorkspaceApplicationKey];
    [self runOnMain:^{
        if (!self.sessionActive || !self.panel) {
            return;
        }
        if (self.pillView.dragging) {
            return;
        }
        if (![app.bundleIdentifier isEqualToString:kSystemSettingsBundleID]) {
            if (self.panel.isVisible) {
                [self.panel orderOut:nil];
            }
            return;
        }
        [self syncWithSystemSettings];
    }];
}

- (void)handleWorkspaceDidDeactivateApp:(NSNotification *)notification {
    NSRunningApplication *app = notification.userInfo[NSWorkspaceApplicationKey];
    [self runOnMain:^{
        if (!self.sessionActive || !self.panel) {
            return;
        }
        if (self.pillView.dragging) {
            return;
        }
        if ([app.bundleIdentifier isEqualToString:kSystemSettingsBundleID] && self.panel.isVisible) {
            [self.panel orderOut:nil];
        }
    }];
}

- (void)handleWorkspaceDidHideApp:(NSNotification *)notification {
    NSRunningApplication *app = notification.userInfo[NSWorkspaceApplicationKey];
    [self runOnMain:^{
        if (!self.sessionActive || !self.panel) {
            return;
        }
        if (self.pillView.dragging) {
            return;
        }
        if ([app.bundleIdentifier isEqualToString:kSystemSettingsBundleID] && self.panel.isVisible) {
            [self.panel orderOut:nil];
        }
    }];
}

- (void)handleWorkspaceDidUnhideApp:(NSNotification *)notification {
    NSRunningApplication *app = notification.userInfo[NSWorkspaceApplicationKey];
    [self runOnMain:^{
        if (!self.sessionActive || !self.panel) {
            return;
        }
        if ([app.bundleIdentifier isEqualToString:kSystemSettingsBundleID]) {
            [self syncWithSystemSettings];
        }
    }];
}

- (void)handleWorkspaceDidTerminateApp:(NSNotification *)notification {
    NSRunningApplication *app = notification.userInfo[NSWorkspaceApplicationKey];
    [self runOnMain:^{
        if (!self.sessionActive) {
            return;
        }
        if ([app.bundleIdentifier isEqualToString:kSystemSettingsBundleID] && self.sawSettingsWindow) {
            [self hideHud];
        }
    }];
}

- (void)handleWorkspaceActiveSpaceDidChange:(NSNotification *)notification {
    (void)notification;
    [self runOnMain:^{
        if (!self.sessionActive || !self.panel) {
            return;
        }
        if (self.pillView.dragging) {
            return;
        }
        [self syncWithSystemSettings];
    }];
}

- (void)handleUserClose:(id)sender {
    (void)sender;
    atomic_fetch_add(&g_user_closed_seq, 1);
    [self hideHud];
}

- (void)handleDragCompleted {
    int32_t draggedKind = self.sessionActive ? self.sessionKind : atomic_load(&g_hud_kind);
    if (draggedKind >= 0) {
        atomic_store(&g_last_dragged_kind, draggedKind);
    }
    atomic_fetch_add(&g_drag_event_seq, 1);
    [self hideHud];
}

- (void)syncWithSystemSettings {
    if (!self.sessionActive || !self.panel) {
        return;
    }
    if (self.pillView.dragging) {
        // Never hide or shift the HUD mid-drag.
        return;
    }

    CGWindowID hudWinNum =
        (self.panel.windowNumber > 0) ? (CGWindowID)self.panel.windowNumber : kCGNullWindowID;
    FabushiSettingsWindowState st = fabushi_inspect_system_settings(hudWinNum);

    NSTimeInterval now = [NSDate timeIntervalSinceReferenceDate];

    if (self.sawSettingsWindow) {
        if (!st.running) {
            [self hideHud];
            return;
        }
        if (!st.hasOnScreenWindow && !st.hidden && !st.hasAnyWindow) {
            if (self.panel.isVisible) {
                [self.panel orderOut:nil];
            }
            if (self.missingWindowSince <= 0.0) {
                self.missingWindowSince = now;
            } else if (now - self.missingWindowSince >= 0.40) {
                // System Settings window was closed (not minimized, hidden, or on another Space).
                [self hideHud];
            }
            return;
        }
        self.missingWindowSince = 0.0;
    } else {
        // Grace period while System Settings launches after clicking "Open in Settings".
        if (now - self.sessionStartedAt > 30.0) {
            [self hideHud];
            return;
        }
    }

    BOOL shouldShowOnScreen =
        st.running &&
        !st.hidden &&
        st.frontmost &&
        st.hasOnScreenWindow &&
        !st.coveredByOtherApp;

    if (shouldShowOnScreen) {
        self.sawSettingsWindow = YES;
        self.missingWindowSince = 0.0;
        self.pillView.cachedSettingsFrame = st.frame;
        self.pillView.cachedSettingsFrameValid = YES;

        NSPoint nextOrigin = fabushi_compute_hud_origin(st.frame, YES);
        NSPoint curOrigin = self.panel.frame.origin;
        if (!self.panel.isVisible ||
            fabs(nextOrigin.x - curOrigin.x) >= 1.0 ||
            fabs(nextOrigin.y - curOrigin.y) >= 1.0) {
            [self.panel setFrameOrigin:nextOrigin];
        }
        if (!self.panel.isVisible) {
            [self.panel orderFrontRegardless];
            [self.panel invalidateShadow];
        }
    } else {
        if (self.panel.isVisible) {
            [self.panel orderOut:nil];
        }
    }
}

- (void)showWithKind:(int32_t)kind
             appName:(NSString *)appName
             appPath:(NSString *)appPath
         instruction:(NSString *)instruction
            dragHint:(NSString *)dragHint {
    [self ensurePanel];
    [self registerWorkspaceObserversIfNeeded];

    self.instructionLabel.stringValue = instruction ?: @"";
    [self.pillView updateWithAppName:appName appPath:appPath dragHint:dragHint];
    self.pillView.cachedSettingsFrame = NSZeroRect;
    self.pillView.cachedSettingsFrameValid = NO;

    self.sessionActive = YES;
    self.sessionKind = kind;
    self.sawSettingsWindow = NO;
    self.missingWindowSince = 0.0;
    self.sessionStartedAt = [NSDate timeIntervalSinceReferenceDate];

    atomic_store(&g_hud_kind, kind);
    atomic_store(&g_hud_visible, true);

    // Only order on-screen once System Settings is actually on-screen & frontmost (no corner flash).
    [self syncWithSystemSettings];

    [self.syncTimer invalidate];
    __weak typeof(self) weakSelf = self;
    self.syncTimer = [NSTimer timerWithTimeInterval:0.10
                                            repeats:YES
                                              block:^(NSTimer * _Nonnull timer) {
        (void)timer;
        [weakSelf syncWithSystemSettings];
    }];
    [[NSRunLoop mainRunLoop] addTimer:self.syncTimer forMode:NSRunLoopCommonModes];
}

- (void)hideHud {
    self.sessionActive = NO;
    self.sessionKind = -1;
    self.sawSettingsWindow = NO;
    self.missingWindowSince = 0.0;
    self.pillView.cachedSettingsFrame = NSZeroRect;
    self.pillView.cachedSettingsFrameValid = NO;

    [self.syncTimer invalidate];
    self.syncTimer = nil;
    if (self.panel) {
        [self.panel orderOut:nil];
    }
    atomic_store(&g_hud_visible, false);
    atomic_store(&g_hud_kind, -1);
}

@end

void fabushi_permission_hud_show(
    int32_t kind,
    const char *app_name,
    const char *app_path,
    const char *instruction_text,
    const char *drag_hint_text
) {
    @autoreleasepool {
        uint64_t gen = atomic_fetch_add(&g_hud_command_gen, 1) + 1;
        NSString *nsAppName = app_name ? [NSString stringWithUTF8String:app_name] : @"Fabushi";
        NSString *nsAppPath = app_path ? [NSString stringWithUTF8String:app_path] : @"";
        NSString *nsInstruction = instruction_text ? [NSString stringWithUTF8String:instruction_text] : @"";
        NSString *nsHint = drag_hint_text ? [NSString stringWithUTF8String:drag_hint_text] : @"";

        dispatch_block_t work = ^{
            @autoreleasepool {
                if (!NSApp || atomic_load(&g_hud_command_gen) != gen) {
                    return;
                }
                [[FabushiPermissionHudController shared] showWithKind:kind
                                                              appName:nsAppName
                                                              appPath:nsAppPath
                                                          instruction:nsInstruction
                                                             dragHint:nsHint];
            }
        };

        if ([NSThread isMainThread]) {
            work();
        } else {
            dispatch_async(dispatch_get_main_queue(), work);
        }
    }
}

void fabushi_permission_hud_hide(void) {
    @autoreleasepool {
        uint64_t gen = atomic_fetch_add(&g_hud_command_gen, 1) + 1;
        dispatch_block_t work = ^{
            @autoreleasepool {
                if (!NSApp || atomic_load(&g_hud_command_gen) != gen) {
                    return;
                }
                [[FabushiPermissionHudController shared] hideHud];
            }
        };

        if ([NSThread isMainThread]) {
            work();
        } else {
            dispatch_async(dispatch_get_main_queue(), work);
        }
    }
}

bool fabushi_permission_hud_is_visible(void) {
    return atomic_load(&g_hud_visible);
}

int32_t fabushi_permission_hud_active_kind(void) {
    return atomic_load(&g_hud_kind);
}

uint64_t fabushi_permission_hud_drag_event_seq(void) {
    return atomic_load(&g_drag_event_seq);
}

int32_t fabushi_permission_hud_last_dragged_kind(void) {
    return atomic_load(&g_last_dragged_kind);
}

uint64_t fabushi_permission_hud_user_closed_seq(void) {
    return atomic_load(&g_user_closed_seq);
}
