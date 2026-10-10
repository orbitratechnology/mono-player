//! Win32 child window for LibVLC.
//!
//! The child returns `HTTRANSPARENT` from `WM_NCHITTEST`, so pointer input over the video goes to
//! the GPUI parent window (same thread). Any mouse message that still arrives is forwarded to the
//! parent with translated coordinates.

use std::sync::OnceLock;

use raw_window_handle::RawWindowHandle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH, MapWindowPoints};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetParent, HTTRANSPARENT, IDC_ARROW,
    LoadCursorW, RegisterClassW, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW,
    SetWindowPos, ShowWindow, WM_MOUSEFIRST, WM_MOUSELAST, WM_NCHITTEST, WNDCLASSW, WS_CHILD,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_NOPARENTNOTIFY,
};

use super::types::{Frame, VideoTarget};

const CLASS_NAME: &[u16] = &[
    b'M' as u16,
    b'o' as u16,
    b'n' as u16,
    b'o' as u16,
    b'V' as u16,
    b'i' as u16,
    b'd' as u16,
    b'e' as u16,
    b'o' as u16,
    0,
];

pub struct Surface {
    hwnd: HWND,
}

impl Surface {
    pub fn create(parent: RawWindowHandle) -> Result<Self, String> {
        let RawWindowHandle::Win32(handle) = parent else {
            return Err(format!("unexpected window handle {parent:?}"));
        };
        let parent = handle.hwnd.get() as HWND;
        register_class()?;
        // SAFETY: plain Win32 call with a registered class and a live parent window owned by
        // this (UI) thread.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOPARENTNOTIFY,
                CLASS_NAME.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                parent,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err("CreateWindowExW failed".into());
        }
        Ok(Self { hwnd })
    }

    pub fn target(&self) -> VideoTarget {
        VideoTarget::Hwnd(self.hwnd as isize)
    }

    pub fn set_frame(&mut self, frame: Frame) {
        let (x, y, width, height) = frame.to_device();
        // SAFETY: `hwnd` is a live child window owned by this thread.
        unsafe {
            SetWindowPos(
                self.hwnd,
                std::ptr::null_mut(),
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        // SAFETY: `hwnd` is a live child window owned by this thread.
        unsafe {
            ShowWindow(self.hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: destroying the window this struct created, on its owning thread.
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

fn register_class() -> Result<(), String> {
    static ATOM: OnceLock<u16> = OnceLock::new();
    let atom = *ATOM.get_or_init(|| {
        // SAFETY: registering a window class with static name and a valid window procedure.
        unsafe {
            let class = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(std::ptr::null()),
                hIcon: std::ptr::null_mut(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: GetStockObject(BLACK_BRUSH) as HBRUSH,
                lpszMenuName: std::ptr::null(),
                lpszClassName: CLASS_NAME.as_ptr(),
            };
            RegisterClassW(&class)
        }
    });
    if atom == 0 {
        Err("RegisterClassW failed".into())
    } else {
        Ok(())
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCHITTEST {
        return HTTRANSPARENT as LRESULT;
    }
    if (WM_MOUSEFIRST..=WM_MOUSELAST).contains(&message) {
        // SAFETY: forwarding to our live parent on the same thread.
        unsafe {
            let parent = GetParent(hwnd);
            if !parent.is_null() {
                let mut point = POINT {
                    x: (lparam & 0xFFFF) as i16 as i32,
                    y: ((lparam >> 16) & 0xFFFF) as i16 as i32,
                };
                MapWindowPoints(hwnd, parent, &mut point, 1);
                let lparam = ((point.y as u16 as isize) << 16) | (point.x as u16 as isize);
                return SendMessageW(parent, message, wparam, lparam);
            }
        }
    }
    // SAFETY: default handling for every other message.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
