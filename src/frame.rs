//! Restore native frames before Rune lays out the first frame.

use rune::WindowHandle;

pub(crate) fn size(name: &str, fallback: (f32, f32)) -> (f32, f32) {
    if isolated() {
        fallback
    } else {
        rune_mac::saved_size(name).unwrap_or(fallback)
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn restore(handle: &WindowHandle, name: &str) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    if isolated() {
        return;
    }
    let (Some(number), Some(main)) = (handle.window_number(), MainThreadMarker::new()) else {
        return;
    };
    let app = NSApplication::sharedApplication(main);
    let Ok(number) = isize::try_from(number) else {
        return;
    };
    let Some(window) = app.windowWithWindowNumber(number) else {
        return;
    };
    let native_name = NSString::from_str(name);
    window.setFrameUsingName(&native_name);
    rune_mac::remember_frame(name);
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn restore(_: &WindowHandle, _: &str) {}

/// Whether Rune's storage was redirected to a separate directory.
pub(crate) fn isolated() -> bool {
    ["RUNE_PREFS_DIR", "RUNE_DIRS_ROOT"]
        .into_iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
}
