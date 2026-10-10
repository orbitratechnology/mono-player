//! X11 child window for LibVLC (Linux/BSD, native X11 or XWayland).
//!
//! Two sibling children of the GPUI window share the video rectangle:
//!
//! - an `InputOutput` window LibVLC draws into, and
//! - an `InputOnly` window stacked above it that selects no events.
//!
//! LibVLC's X11 embedding always selects `PointerMotion` on its drawable (for cursor
//! auto-hiding), and X stops propagating an event at the first window where any client selected
//! it. The invisible input window sits above the video instead, so pointer motion, clicks and
//! scrolling propagate from it straight to the GPUI window (used, for example, to reveal the
//! fullscreen controls). LibVLC is also told not to handle input (`set_mouse_input(false)`).
//!
//! All requests here are fire-and-forget except one geometry query at creation, so the UI thread
//! never waits on the X server during resizes.

use raw_window_handle::RawWindowHandle;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT};

use super::types::{Frame, VideoTarget};

pub struct Surface {
    connection: RustConnection,
    /// LibVLC's drawable.
    window: u32,
    /// Transparent to rendering, forwards input to the GPUI window.
    input_window: u32,
}

impl Surface {
    pub fn create(parent: RawWindowHandle) -> Result<Self, String> {
        let parent = match parent {
            RawWindowHandle::Xlib(handle) => handle.window as u32,
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            RawWindowHandle::Wayland(_) => {
                return Err(
                    "native Wayland is not supported for embedded video; Mono Player runs under \
                     XWayland when DISPLAY is set"
                        .into(),
                );
            }
            other => return Err(format!("unexpected window handle {other:?}")),
        };
        let (connection, screen_index) =
            RustConnection::connect(None).map_err(|e| format!("X11 connect failed: {e}"))?;
        let black = connection
            .setup()
            .roots
            .get(screen_index)
            .map(|screen| screen.black_pixel)
            .unwrap_or(0);
        // One round trip at creation: a 32-bit (ARGB) parent needs an opaque black background.
        let depth = connection
            .get_geometry(parent)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| format!("parent window unavailable: {e}"))?
            .depth;
        let background = if depth == 32 { 0xFF00_0000 } else { black };

        let window = connection.generate_id().map_err(|e| e.to_string())?;
        connection
            .create_window(
                COPY_DEPTH_FROM_PARENT,
                window,
                parent,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                COPY_FROM_PARENT,
                &CreateWindowAux::new()
                    .background_pixel(background)
                    // No input events: they propagate to the GPUI window.
                    .event_mask(EventMask::NO_EVENT),
            )
            .map_err(|e| e.to_string())?;
        // Created second, so it is stacked above the video window.
        let input_window = connection.generate_id().map_err(|e| e.to_string())?;
        connection
            .create_window(
                0,
                input_window,
                parent,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                COPY_FROM_PARENT,
                &CreateWindowAux::new().event_mask(EventMask::NO_EVENT),
            )
            .map_err(|e| e.to_string())?;
        connection.flush().map_err(|e| e.to_string())?;
        Ok(Self {
            connection,
            window,
            input_window,
        })
    }

    pub fn target(&self) -> VideoTarget {
        VideoTarget::X11Window(self.window)
    }

    pub fn set_frame(&mut self, frame: Frame) {
        let (x, y, width, height) = frame.to_device();
        let geometry = ConfigureWindowAux::new()
            .x(x)
            .y(y)
            .width(width)
            .height(height);
        let _ = self.connection.configure_window(self.window, &geometry);
        let _ = self
            .connection
            .configure_window(self.input_window, &geometry);
        self.flush();
    }

    pub fn set_visible(&mut self, visible: bool) {
        for window in [self.window, self.input_window] {
            let _ = if visible {
                self.connection.map_window(window)
            } else {
                self.connection.unmap_window(window)
            };
        }
        self.flush();
    }

    fn flush(&self) {
        let _ = self.connection.flush();
        // Drain asynchronous errors (e.g. the parent vanished) without blocking.
        while let Ok(Some(_)) = self.connection.poll_for_event() {}
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        let _ = self.connection.destroy_window(self.input_window);
        let _ = self.connection.destroy_window(self.window);
        let _ = self.connection.flush();
    }
}
