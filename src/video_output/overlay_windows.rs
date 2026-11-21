//! Windows: the controls window becomes an owned, non-topmost popup of the player window.
//!
//! Owned windows always stay above their owner and minimize with it; DWM composites them, so
//! the controls can be translucent. Positions are converted from the owner's client area.

use raw_window_handle::RawWindowHandle;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWLP_HWNDPARENT, HWND_NOTOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow,
};

use super::types::Frame;

pub struct Host {
    main: HWND,
    overlay: HWND,
}

fn hwnd(handle: RawWindowHandle) -> Result<HWND, String> {
    match handle {
        RawWindowHandle::Win32(h) => Ok(h.hwnd.get() as HWND),
        other => Err(format!("not a Win32 window: {other:?}")),
    }
}

impl Host {
    pub fn attach(main: RawWindowHandle, overlay: RawWindowHandle) -> Result<Self, String> {
        let (main, overlay) = (hwnd(main)?, hwnd(overlay)?);
        // SAFETY: both are live top-level windows owned by this (UI) thread.
        unsafe {
            SetWindowLongPtrW(overlay, GWLP_HWNDPARENT, main as isize);
            // GPUI makes pop-ups topmost; an owned window only needs to sit above its owner.
            SetWindowPos(
                overlay,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        Ok(Self { main, overlay })
    }

    pub fn set_frame(&mut self, frame: Frame, _corner_radius: f32) {
        let (x, y, width, height) = frame.to_device();
        let mut origin = POINT { x, y };
        // SAFETY: live windows on their owning thread.
        unsafe {
            ClientToScreen(self.main, &mut origin);
            SetWindowPos(
                self.overlay,
                std::ptr::null_mut(),
                origin.x,
                origin.y,
                width as i32,
                height as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        // SAFETY: live window on its owning thread.
        unsafe {
            ShowWindow(
                self.overlay,
                if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
            );
        }
    }
}
