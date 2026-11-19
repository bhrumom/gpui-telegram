//! Native macOS privacy permission checks and System Settings registration/deep-links.
//! Calling `open_screen_recording_settings()` or `open_accessibility_settings()` invokes
//! the official macOS registration APIs (`CGRequestScreenCaptureAccess` /
//! `AXIsProcessTrustedWithOptions` with `kAXTrustedCheckOptionPrompt = true`) so that
//! `Fabushi` appears directly inside macOS System Settings with its icon and toggle switch.

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    #[link(name = "ApplicationServices", kind = "framework")]
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;

        static kAXTrustedCheckOptionPrompt: *const c_void;
        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;

        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }

    pub fn has_screen_recording_permission() -> bool {
        if cfg!(test) || std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
            return true;
        }
        unsafe { CGPreflightScreenCaptureAccess() }
    }

    pub fn has_accessibility_permission() -> bool {
        if cfg!(test) || std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
            return true;
        }
        unsafe { AXIsProcessTrusted() }
    }

    /// Registers `Fabushi` in macOS System Settings -> Screen Recording and opens that pane
    /// so the user can directly toggle the switch ON.
    pub fn open_screen_recording_settings() {
        if cfg!(test) || std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
            return;
        }
        unsafe {
            let _ = CGRequestScreenCaptureAccess();
        }
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
            .spawn();
    }

    /// Registers `Fabushi` in macOS System Settings -> Accessibility (via `kAXTrustedCheckOptionPrompt`)
    /// and opens that pane so the user can directly toggle the switch ON.
    pub fn open_accessibility_settings() {
        if cfg!(test) || std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
            return;
        }
        unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let values = [kCFBooleanTrue];
            let opts = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks as *const c_void,
                &kCFTypeDictionaryValueCallBacks as *const c_void,
            );
            if !opts.is_null() {
                let _ = AXIsProcessTrustedWithOptions(opts);
                CFRelease(opts);
            }
        }
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn has_screen_recording_permission() -> bool {
        true
    }
    pub fn has_accessibility_permission() -> bool {
        true
    }
    pub fn open_screen_recording_settings() {}
    pub fn open_accessibility_settings() {}
}

pub use imp::{
    has_accessibility_permission, has_screen_recording_permission, open_accessibility_settings,
    open_screen_recording_settings,
};
