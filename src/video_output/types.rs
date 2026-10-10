//! Plain data shared by the platform surfaces and the playback actor (no GPUI or LibVLC types).

/// A native drawable LibVLC can render into. Plain integers so it can cross to the playback
/// thread; the UI guarantees the drawable outlives its use (see [`super::VideoSurface`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoTarget {
    /// An X11 window id (Linux/BSD under X11 or XWayland).
    X11Window(u32),
    /// A Win32 `HWND`.
    Hwnd(isize),
    /// An AppKit `NSView*`.
    NsView(usize),
}

/// The video rectangle in the parent window, in logical pixels, plus the window scale factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

impl Frame {
    /// The frame in device pixels, rounded outward so no gap appears at fractional scales.
    pub fn to_device(self) -> (i32, i32, u32, u32) {
        let x = (self.x * self.scale).floor();
        let y = (self.y * self.scale).floor();
        let right = ((self.x + self.width) * self.scale).ceil();
        let bottom = ((self.y + self.height) * self.scale).ceil();
        (
            x as i32,
            y as i32,
            (right - x).max(1.0) as u32,
            (bottom - y).max(1.0) as u32,
        )
    }

    /// Whether the frame has no visible area.
    pub fn is_empty(self) -> bool {
        self.width < 1.0 || self.height < 1.0
    }
}
