#import <Cocoa/Cocoa.h>
#import <CoreGraphics/CoreGraphics.h>
#import <QuartzCore/QuartzCore.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>

static const CGFloat kHudWidth = 304.0;
static const CGFloat kHudHeight = 114.0;

static _Atomic uint64_t g_drag_event_seq = 0;
static _Atomic uint64_t g_user_closed_seq = 0;
static _Atomic bool g_hud_visible = false;
static _Atomic int32_t g_hud_kind = -1;

static BOOL fabushi_find_system_settings_frame(NSRect *outFrame, BOOL *outRunning) {
    NSArray<NSRunningApplication *> *apps =
        [NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.apple.systempreferences"];
    if (outRunning) {
        *outRunning = (apps.count > 0);
    }
    if (apps.count == 0) {
        return NO;
    }

    pid_t settingsPid = apps.firstObject.processIdentifier;
    CFArrayRef winList = CGWindowListCopyWindowInfo(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID
    );
    if (!winList) {
        return NO;
    }

    NSArray *windows = CFBridgingRelease(winList);
    CGFloat primaryHeight = 0.0;
    NSScreen *primaryScreen = NSScreen.screens.firstObject ?: [NSScreen mainScreen];
    if (primaryScreen) {
        primaryHeight = primaryScreen.frame.size.height;
    }
    if (primaryHeight <= 0.0) {
        primaryHeight = 900.0;
    }

    CGRect bestRect = CGRectZero;
    CGFloat bestArea = 0.0;

    for (NSDictionary *win in windows) {
        NSNumber *pidNum = win[(__bridge NSString *)kCGWindowOwnerPID];
        if (!pidNum || pidNum.intValue != settingsPid) {
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
        if (cgRect.size.width < 320.0 || cgRect.size.height < 280.0) {
            continue;
        }
        CGFloat area = cgRect.size.width * cgRect.size.height;
        if (area > bestArea) {
            bestArea = area;
            bestRect = cgRect;
        }
    }

    if (bestArea <= 0.0) {
        return NO;
    }

    if (outFrame) {
        // Convert CoreGraphics top-left coordinates to AppKit bottom-left coordinates
        CGFloat cocoaY = primaryHeight - bestRect.origin.y - bestRect.size.height;
        *outFrame = NSMakeRect(bestRect.origin.x, cocoaY, bestRect.size.width, bestRect.size.height);
    }
    return YES;
}

static NSPoint fabushi_compute_hud_origin(NSRect settingsFrame, BOOL hasSettingsFrame) {
    NSScreen *targetScreen = [NSScreen mainScreen] ?: NSScreen.screens.firstObject;
    if (hasSettingsFrame) {
        NSPoint settingsCenter = NSMakePoint(NSMidX(settingsFrame), NSMidY(settingsFrame));
        for (NSScreen *scr in NSScreen.screens) {
            if (NSPointInRect(settingsCenter, scr.frame)) {
                targetScreen = scr;
                break;
            }
        }
    }

    NSRect visible = targetScreen ? targetScreen.visibleFrame : NSMakeRect(0, 0, 1440, 900);
    CGFloat x = 0.0;
    CGFloat y = 0.0;

    if (hasSettingsFrame) {
        // Anchor near the bottom of the System Settings detail pane (matching Gemini HUD placement)
        CGFloat detailCenterX = settingsFrame.origin.x + settingsFrame.size.width * 0.62;
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

@interface FabushiDraggableAppPillView : NSView <NSDraggingSource>
@property (nonatomic, copy) NSString *appPath;
@property (nonatomic, copy) NSString *appName;
@property (nonatomic, copy) NSString *dragHint;
@property (nonatomic, assign) BOOL hovered;
@property (nonatomic, assign) BOOL dragging;
@property (nonatomic, assign) NSPoint mouseDownLocal;
@property (nonatomic, assign) NSPoint mouseDownScreen;
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
        _nameLabel.frame = NSMakeRect(42.0, 9.0, 146.0, 20.0);
        [self addSubview:_nameLabel];

        _hintLabel = [NSTextField labelWithString:@"按住拖入"];
        _hintLabel.font = [NSFont systemFontOfSize:11.0 weight:NSFontWeightMedium];
        _hintLabel.textColor = [NSColor secondaryLabelColor];
        _hintLabel.alignment = NSTextAlignmentRight;
        _hintLabel.frame = NSMakeRect(190.0, 10.0, frameRect.size.width - 200.0, 18.0);
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
    CGFloat fillAlpha = self.dragging ? 0.14 : (self.hovered ? 0.10 : 0.055);
    self.layer.backgroundColor = [[NSColor labelColor] colorWithAlphaComponent:fillAlpha].CGColor;
    self.layer.borderColor = [[NSColor separatorColor] colorWithAlphaComponent:self.hovered ? 0.65 : 0.35].CGColor;
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
    NSTrackingAreaOptions opts = NSTrackingMouseEnteredAndExited | NSTrackingActiveAlways | NSTrackingInVisibleRect;
    self.trackingArea = [[NSTrackingArea alloc] initWithRect:self.bounds options:opts owner:self userInfo:nil];
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
    [self beginDraggingSessionWithItems:@[item] event:event source:self];
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

    BOOL droppedIntoSettings = (operation != NSDragOperationNone);
    if (!droppedIntoSettings) {
        NSRect settingsFrame = NSZeroRect;
        BOOL settingsRunning = NO;
        if (fabushi_find_system_settings_frame(&settingsFrame, &settingsRunning) &&
            NSPointInRect(screenPoint, settingsFrame)) {
            droppedIntoSettings = YES;
        }
    }
    if (!droppedIntoSettings) {
        CGFloat ddx = screenPoint.x - self.mouseDownScreen.x;
        CGFloat ddy = screenPoint.y - self.mouseDownScreen.y;
        if (ddx * ddx + ddy * ddy > 1600.0) {
            droppedIntoSettings = YES;
        }
    }

    if (droppedIntoSettings) {
        atomic_fetch_add(&g_drag_event_seq, 1);
    }
}

@end

@interface FabushiPermissionHudController : NSObject
@property (nonatomic, strong) FabushiPermissionHudPanel *panel;
@property (nonatomic, strong) NSVisualEffectView *backdrop;
@property (nonatomic, strong) NSView *borderOverlay;
@property (nonatomic, strong) NSTextField *instructionLabel;
@property (nonatomic, strong) FabushiDraggableAppPillView *pillView;
@property (nonatomic, strong) NSTimer *anchorTimer;
@property (nonatomic, assign) NSInteger anchorTicks;
@property (nonatomic, assign) BOOL sawSettingsWindow;
@property (nonatomic, assign) NSPoint lastAnchoredOrigin;
@end

@implementation FabushiPermissionHudController

+ (instancetype)shared {
    static FabushiPermissionHudController *instance = nil;
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        instance = [[FabushiPermissionHudController alloc] init];
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
    panel.level = NSFloatingWindowLevel + 1;
    panel.collectionBehavior =
        NSWindowCollectionBehaviorCanJoinAllSpaces |
        NSWindowCollectionBehaviorFullScreenAuxiliary |
        NSWindowCollectionBehaviorStationary;
    panel.opaque = NO;
    panel.backgroundColor = [NSColor clearColor];
    panel.hasShadow = YES;
    panel.movableByWindowBackground = YES;
    panel.releasedWhenClosed = NO;

    NSVisualEffectView *backdrop = [[NSVisualEffectView alloc] initWithFrame:NSMakeRect(0, 0, kHudWidth, kHudHeight)];
    backdrop.material = NSVisualEffectMaterialPopover;
    backdrop.blendingMode = NSVisualEffectBlendingModeBehindWindow;
    backdrop.state = NSVisualEffectStateActive;
    backdrop.wantsLayer = YES;
    backdrop.layer.cornerRadius = 16.0;
    backdrop.layer.masksToBounds = YES;

    NSView *borderOverlay = [[NSView alloc] initWithFrame:backdrop.bounds];
    borderOverlay.wantsLayer = YES;
    borderOverlay.layer.cornerRadius = 16.0;
    borderOverlay.layer.borderWidth = 1.0;
    borderOverlay.layer.borderColor = [[NSColor separatorColor] colorWithAlphaComponent:0.55].CGColor;
    [backdrop addSubview:borderOverlay];

    // Up arrow icon matching Gemini HUD
    NSView *arrowView = nil;
    if (@available(macOS 11.0, *)) {
        NSImage *sym = [NSImage imageWithSystemSymbolName:@"arrow.up" accessibilityDescription:nil];
        if (sym) {
            NSImageSymbolConfiguration *cfg = [NSImageSymbolConfiguration configurationWithPointSize:13.0 weight:NSFontWeightMedium];
            sym = [sym imageWithSymbolConfiguration:cfg];
            NSImageView *iv = [[NSImageView alloc] initWithFrame:NSMakeRect(14.0, 74.0, 16.0, 18.0)];
            iv.image = sym;
            iv.contentTintColor = [NSColor secondaryLabelColor];
            arrowView = iv;
        }
    }
    if (!arrowView) {
        NSTextField *arrowLbl = [NSTextField labelWithString:@"↑"];
        arrowLbl.font = [NSFont systemFontOfSize:13.5 weight:NSFontWeightMedium];
        arrowLbl.textColor = [NSColor secondaryLabelColor];
        arrowLbl.frame = NSMakeRect(14.0, 74.0, 16.0, 18.0);
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
    instruction.frame = NSMakeRect(36.0, 58.0, 232.0, 42.0);
    [backdrop addSubview:instruction];

    // Close button top-right
    NSButton *closeBtn = [[NSButton alloc] initWithFrame:NSMakeRect(kHudWidth - 30.0, kHudHeight - 30.0, 22.0, 22.0)];
    closeBtn.bordered = NO;
    closeBtn.bezelStyle = NSBezelStyleRegularSquare;
    closeBtn.target = self;
    closeBtn.action = @selector(handleUserClose:);
    if (@available(macOS 11.0, *)) {
        NSImage *xSym = [NSImage imageWithSystemSymbolName:@"xmark" accessibilityDescription:@"Close"];
        if (xSym) {
            NSImageSymbolConfiguration *cfg = [NSImageSymbolConfiguration configurationWithPointSize:10.5 weight:NSFontWeightSemibold];
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
    [backdrop addSubview:pill];

    panel.contentView = backdrop;

    self.panel = panel;
    self.backdrop = backdrop;
    self.borderOverlay = borderOverlay;
    self.instructionLabel = instruction;
    self.pillView = pill;
}

- (void)handleUserClose:(id)sender {
    (void)sender;
    atomic_fetch_add(&g_user_closed_seq, 1);
    [self hideHud];
}

- (void)repositionIfAppropriate {
    if (!self.panel || !self.panel.isVisible) {
        return;
    }
    NSRect settingsFrame = NSZeroRect;
    BOOL settingsRunning = NO;
    BOOL hasFrame = fabushi_find_system_settings_frame(&settingsFrame, &settingsRunning);

    if (hasFrame) {
        self.sawSettingsWindow = YES;
        // Stop forcing position if the user manually dragged the HUD panel elsewhere
        NSPoint currentOrigin = self.panel.frame.origin;
        CGFloat movedDist = hypot(currentOrigin.x - self.lastAnchoredOrigin.x,
                                  currentOrigin.y - self.lastAnchoredOrigin.y);
        if (self.anchorTicks <= 10 || movedDist < 8.0) {
            NSPoint nextOrigin = fabushi_compute_hud_origin(settingsFrame, YES);
            if (hypot(nextOrigin.x - currentOrigin.x, nextOrigin.y - currentOrigin.y) > 2.0) {
                [self.panel setFrameOrigin:nextOrigin];
            }
            self.lastAnchoredOrigin = nextOrigin;
        }
    } else if (self.sawSettingsWindow && !settingsRunning) {
        // System Settings was closed by user -> cleanly dismiss floating HUD
        [self hideHud];
        return;
    }

    self.anchorTicks += 1;
}

- (void)showWithKind:(int32_t)kind
             appName:(NSString *)appName
             appPath:(NSString *)appPath
         instruction:(NSString *)instruction
            dragHint:(NSString *)dragHint {
    [self ensurePanel];

    self.instructionLabel.stringValue = instruction ?: @"";
    [self.pillView updateWithAppName:appName appPath:appPath dragHint:dragHint];

    NSRect settingsFrame = NSZeroRect;
    BOOL settingsRunning = NO;
    BOOL hasFrame = fabushi_find_system_settings_frame(&settingsFrame, &settingsRunning);
    NSPoint origin = fabushi_compute_hud_origin(settingsFrame, hasFrame);
    [self.panel setFrameOrigin:origin];
    self.lastAnchoredOrigin = origin;
    self.sawSettingsWindow = hasFrame;
    self.anchorTicks = 0;

    [self.panel orderFrontRegardless];
    atomic_store(&g_hud_kind, kind);
    atomic_store(&g_hud_visible, true);

    [self.anchorTimer invalidate];
    __weak typeof(self) weakSelf = self;
    self.anchorTimer = [NSTimer scheduledTimerWithTimeInterval:0.35
                                                       repeats:YES
                                                         block:^(NSTimer * _Nonnull timer) {
        (void)timer;
        [weakSelf repositionIfAppropriate];
    }];
}

- (void)hideHud {
    [self.anchorTimer invalidate];
    self.anchorTimer = nil;
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
    NSString *nsAppName = app_name ? [NSString stringWithUTF8String:app_name] : @"Fabushi";
    NSString *nsAppPath = app_path ? [NSString stringWithUTF8String:app_path] : @"";
    NSString *nsInstruction = instruction_text ? [NSString stringWithUTF8String:instruction_text] : @"";
    NSString *nsHint = drag_hint_text ? [NSString stringWithUTF8String:drag_hint_text] : @"";

    dispatch_block_t work = ^{
        @autoreleasepool {
            if (!NSApp) {
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

void fabushi_permission_hud_hide(void) {
    dispatch_block_t work = ^{
        @autoreleasepool {
            if (!NSApp) {
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

bool fabushi_permission_hud_is_visible(void) {
    return atomic_load(&g_hud_visible);
}

int32_t fabushi_permission_hud_active_kind(void) {
    return atomic_load(&g_hud_kind);
}

uint64_t fabushi_permission_hud_drag_event_seq(void) {
    return atomic_load(&g_drag_event_seq);
}

uint64_t fabushi_permission_hud_user_closed_seq(void) {
    return atomic_load(&g_user_closed_seq);
}
