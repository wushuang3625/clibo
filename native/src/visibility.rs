// Hidden windows may not receive a redraw. Reveal the OS window directly from
// the hotkey/menu event, then let eframe reconcile its viewport state.
#[cfg(windows)]
pub fn reveal(handle: usize) {
    if handle == 0 {
        return;
    }
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        ShowWindow(handle as _, SW_SHOWNORMAL);
        SetForegroundWindow(handle as _);
    }
}
#[cfg(target_os = "macos")]
pub fn reveal(_handle: usize) {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut Object = msg_send![app, windows];
        let window: *mut Object = msg_send![windows, firstObject];
        if !window.is_null() {
            let _: () = msg_send![window,makeKeyAndOrderFront:std::ptr::null_mut::<Object>()];
            let _: () = msg_send![app,activateIgnoringOtherApps:true];
        }
    }
}
