//! OS-level unread badge on the application icon.
//!
//! Currently only macOS is implemented (dock tile badge via AppKit).
//! Linux/Windows are no-ops; they can be wired later through Unity
//! Launcher / `SetOverlayIcon` respectively.

/// Set the application badge to `count`. `0` clears the badge.
/// Must be called from the main thread (Iced guarantees this for
/// `App::update`).
pub fn set(count: u32) {
    set_impl(count);
}

#[cfg(target_os = "macos")]
fn set_impl(count: u32) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;

    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let dock_tile: *mut AnyObject = msg_send![app, dockTile];
        if dock_tile.is_null() {
            return;
        }
        if count == 0 {
            let nil: *const AnyObject = std::ptr::null();
            let _: () = msg_send![dock_tile, setBadgeLabel: nil];
        } else {
            let s = NSString::from_str(&count.to_string());
            let _: () = msg_send![dock_tile, setBadgeLabel: &*s];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn set_impl(_count: u32) {}
