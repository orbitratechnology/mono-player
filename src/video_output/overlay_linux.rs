//! X11: the controls window becomes a child of the player window.
//!
//! GPUI cannot draw over the native video child window, so the floating controls are their own
//! GPUI window. Reparenting it into the player window (the XEmbed technique) puts it above the
//! video in the same window: it moves, minimizes and stacks with the player for free, and the
//! window manager stops managing it. X does not alpha-blend sibling windows, so the controls'
//! rounded corners are cut with the Shape extension instead of transparency.

use raw_window_handle::RawWindowHandle;
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ClipOrdering, ConfigureWindowAux, ConnectionExt as _, Rectangle,
    StackMode,
};
use x11rb::rust_connection::RustConnection;

use super::types::Frame;

pub struct Host {
    connection: RustConnection,
    overlay: u32,
    has_shape: bool,
}

fn xid(handle: RawWindowHandle) -> Result<u32, String> {
    match handle {
        RawWindowHandle::Xlib(h) => Ok(h.window as u32),
        RawWindowHandle::Xcb(h) => Ok(h.window.get()),
        other => Err(format!("not an X11 window: {other:?}")),
    }
}

impl Host {
    pub fn attach(main: RawWindowHandle, overlay: RawWindowHandle) -> Result<Self, String> {
        let (main, overlay) = (xid(main)?, xid(overlay)?);
        let (connection, _) =
            RustConnection::connect(None).map_err(|e| format!("X11 connect failed: {e}"))?;
        let has_shape = connection
            .extension_information(shape::X11_EXTENSION_NAME)
            .ok()
            .flatten()
            .is_some();
        // Unmanaged from now on: the window manager must not frame or move it.
        connection
            .change_window_attributes(
                overlay,
                &ChangeWindowAttributesAux::new().override_redirect(1),
            )
            .map_err(|e| e.to_string())?;
        connection
            .reparent_window(overlay, main, 0, 0)
            .map_err(|e| e.to_string())?;
        // Above the video and the input-forwarding window.
        connection
            .configure_window(
                overlay,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .map_err(|e| e.to_string())?;
        connection.flush().map_err(|e| e.to_string())?;
        Ok(Self {
            connection,
            overlay,
            has_shape,
        })
    }

    pub fn set_frame(&mut self, frame: Frame, corner_radius: f32) {
        let (x, y, width, height) = frame.to_device();
        let _ = self.connection.configure_window(
            self.overlay,
            &ConfigureWindowAux::new()
                .x(x)
                .y(y)
                .width(width)
                .height(height)
                .stack_mode(StackMode::ABOVE),
        );
        if self.has_shape {
            let radius = (corner_radius * frame.scale).round() as u32;
            let rectangles = rounded_rect_spans(width, height, radius);
            let _ = self.connection.shape_rectangles(
                shape::SO::SET,
                shape::SK::BOUNDING,
                ClipOrdering::UNSORTED,
                self.overlay,
                0,
                0,
                &rectangles,
            );
        }
        self.flush();
    }

    pub fn set_visible(&mut self, visible: bool) {
        let _ = if visible {
            self.connection.map_window(self.overlay)
        } else {
            self.connection.unmap_window(self.overlay)
        };
        self.flush();
    }

    fn flush(&self) {
        let _ = self.connection.flush();
        while let Ok(Some(_)) = self.connection.poll_for_event() {}
    }
}

/// Rectangles covering a rounded rectangle: one per scanline inside the corner arcs, plus one
/// for the straight middle band.
pub fn rounded_rect_spans(width: u32, height: u32, radius: u32) -> Vec<Rectangle> {
    let radius = radius.min(width / 2).min(height / 2);
    let mut rects = Vec::with_capacity(radius as usize * 2 + 1);
    let r = radius as f32;
    for row in 0..radius {
        // Horizontal inset of the arc at the centre of this scanline.
        let dy = r - row as f32 - 0.5;
        let inset = (r - (r * r - dy * dy).max(0.0).sqrt()).round() as u32;
        let span = width.saturating_sub(inset * 2) as u16;
        for y in [row, height - 1 - row] {
            rects.push(Rectangle {
                x: inset as i16,
                y: y as i16,
                width: span,
                height: 1,
            });
        }
    }
    rects.push(Rectangle {
        x: 0,
        y: radius as i16,
        width: width as u16,
        height: height.saturating_sub(radius * 2) as u16,
    });
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_cover_a_rounded_rect() {
        let rects = rounded_rect_spans(100, 40, 10);
        // 10 top rows + 10 bottom rows + the middle band.
        assert_eq!(rects.len(), 21);
        // The first row is inset the most; the middle band is full width.
        assert!(rects[0].x > 0);
        let middle = rects.last().unwrap();
        assert_eq!(
            (middle.x, middle.y, middle.width, middle.height),
            (0, 10, 100, 20)
        );
        // Rows are symmetric.
        assert_eq!(rects[0].x, rects[1].x);
        assert_eq!(rects[1].y, 39);
    }

    #[test]
    fn radius_is_limited_by_size() {
        let rects = rounded_rect_spans(10, 6, 50);
        assert_eq!(rects.last().unwrap().height, 0);
        assert!(rects.iter().all(|r| r.width <= 10));
    }
}
