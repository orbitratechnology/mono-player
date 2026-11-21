//! macOS: the controls window becomes a borderless child panel of the player window.
//!
//! Child windows move with their parent (also into native fullscreen), stay ordered above it and
//! are composited by the window server, so the controls can be truly translucent.

use std::ffi::c_void;

use objc2::msg_send;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::RawWindowHandle;

use super::types::Frame;

// NSWindowStyleMask / NSWindowOrderingMode / NSWindowLevel values.
const STYLE_BORDERLESS: usize = 0;
const STYLE_NONACTIVATING_PANEL: usize = 1 << 7;
const ORDER_ABOVE: isize = 1;
const NORMAL_LEVEL: isize = 0;

pub struct Host {
    main_view: *mut AnyObject,
    main_window: *mut AnyObject,
    overlay_window: *mut AnyObject,
}

fn view(handle: RawWindowHandle) -> Result<*mut AnyObject, String> {
    match handle {
        RawWindowHandle::AppKit(h) => Ok(h.ns_view.as_ptr().cast::<AnyObject>()),
        other => Err(format!("not an AppKit window: {other:?}")),
    }
}

impl Host {
    pub fn attach(main: RawWindowHandle, overlay: RawWindowHandle) -> Result<Self, String> {
        let (main_view, overlay_view) = (view(main)?, view(overlay)?);
        // SAFETY: both views are live GPUI content views; all calls run on the main thread.
        unsafe {
            let main_window: *mut AnyObject = msg_send![main_view, window];
            let overlay_window: *mut AnyObject = msg_send![overlay_view, window];
            if main_window.is_null() || overlay_window.is_null() {
                return Err("window not available yet".into());
            }
            let _: () = msg_send![
                overlay_window,
                setStyleMask: STYLE_BORDERLESS | STYLE_NONACTIVATING_PANEL
            ];
            let _: () = msg_send![overlay_window, setOpaque: false];
            let _: () = msg_send![overlay_window, setHasShadow: true];
            let _: () = msg_send![overlay_window, setLevel: NORMAL_LEVEL];
            let _: () = msg_send![overlay_window, setHidesOnDeactivate: false];
            let _: () =
                msg_send![main_window, addChildWindow: overlay_window, ordered: ORDER_ABOVE];
            Ok(Self {
                main_view,
                main_window,
                overlay_window,
            })
        }
    }

    pub fn set_frame(&mut self, frame: Frame, _corner_radius: f32) {
        // SAFETY: live windows/views on the main thread.
        unsafe {
            let flipped: bool = msg_send![self.main_view, isFlipped];
            let bounds: NSRect = msg_send![self.main_view, bounds];
            let y = if flipped {
                f64::from(frame.y)
            } else {
                bounds.size.height - f64::from(frame.y) - f64::from(frame.height)
            };
            let in_view = NSRect::new(
                NSPoint::new(f64::from(frame.x), y),
                NSSize::new(f64::from(frame.width), f64::from(frame.height)),
            );
            let nil: *mut c_void = std::ptr::null_mut();
            let in_window: NSRect = msg_send![self.main_view, convertRect: in_view, toView: nil];
            let on_screen: NSRect = msg_send![self.main_window, convertRectToScreen: in_window];
            let _: () = msg_send![self.overlay_window, setFrame: on_screen, display: true];
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        // SAFETY: live window on the main thread.
        unsafe {
            let alpha: f64 = if visible { 1.0 } else { 0.0 };
            let _: () = msg_send![self.overlay_window, setAlphaValue: alpha];
            let _: () = msg_send![self.overlay_window, setIgnoresMouseEvents: !visible];
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // SAFETY: detaching our child from its live parent on the main thread.
        unsafe {
            let _: () = msg_send![self.main_window, removeChildWindow: self.overlay_window];
        }
    }
}
