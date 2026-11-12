//! Where the mouse is, in global screen points, read straight from the OS.
//!
//! During an external file drag, macOS doesn't deliver mouse-move events to the window
//! (winit's hovered/dropped-file events carry no position either), so egui's pointer is
//! stale. Reading the global position lets a drop land on the field it was dropped on.

/// Global mouse position in points, top-left origin of the main display — the same space
/// as egui's `ViewportInfo::inner_rect` on macOS.
#[cfg(target_os = "macos")]
pub fn global() -> Option<(f32, f32)> {
    use std::ffi::c_void;

    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    // SAFETY: CGEventCreate(NULL) returns a new event carrying the current mouse location
    // (or NULL); it needs no permissions. We release what we create.
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let p = CGEventGetLocation(event);
        CFRelease(event);
        Some((p.x as f32, p.y as f32))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn global() -> Option<(f32, f32)> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn global_position_is_finite_when_available() {
        if let Some((x, y)) = super::global() {
            assert!(x.is_finite() && y.is_finite());
        }
    }
}
