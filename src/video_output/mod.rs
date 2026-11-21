//! The native video surface: a platform child view that LibVLC renders into directly.
//!
//! This is the preferred strategy from the PRD (direct native output, no per-frame CPU copies).
//! GPUI cannot draw over a native child surface, so the layout keeps every GPUI control outside
//! the video rectangle; the surface is hidden whenever there is nothing to show (no media,
//! audio-only media) so GPUI can paint that area instead.
//!
//! Lifetime rule: a surface is only dropped after the playback actor has released its player
//! (see `PlayerView::shutdown`), so LibVLC never draws into a destroyed drawable.

mod types;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[path = "linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod platform;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[path = "overlay_linux.rs"]
mod overlay_platform;
#[cfg(target_os = "macos")]
#[path = "overlay_macos.rs"]
mod overlay_platform;
#[cfg(target_os = "windows")]
#[path = "overlay_windows.rs"]
mod overlay_platform;

pub use types::{Frame, VideoTarget};

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "windows"
))]
mod surface {
    use gpui_kit::{Bounds, Pixels, Window};
    use raw_window_handle::HasWindowHandle;

    use super::platform::Surface;
    use super::types::{Frame, VideoTarget};

    /// A native child surface positioned over a rectangle of the GPUI window.
    pub struct VideoSurface {
        surface: Surface,
        frame: Option<Frame>,
        visible: bool,
    }

    impl VideoSurface {
        /// Create a hidden child surface in `window`. Must run on the UI thread.
        pub fn create(window: &Window) -> Result<Self, String> {
            // GPUI's inherent `window_handle()` returns its own id type; ask raw-window-handle.
            let handle = HasWindowHandle::window_handle(window)
                .map_err(|e| format!("no native window handle: {e}"))?;
            let surface = Surface::create(handle.as_raw())?;
            Ok(Self {
                surface,
                frame: None,
                visible: false,
            })
        }

        pub fn target(&self) -> VideoTarget {
            self.surface.target()
        }

        /// Move/resize the surface. A no-op when nothing changed, so it is cheap to call on
        /// every paint.
        pub fn set_bounds(&mut self, bounds: Bounds<Pixels>, scale: f32) {
            let frame = Frame {
                x: f32::from(bounds.origin.x),
                y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
                scale,
            };
            if self.frame != Some(frame) {
                self.frame = Some(frame);
                self.surface.set_frame(frame);
            }
        }

        /// Show or hide the surface. Hidden until a frame has been set.
        pub fn set_visible(&mut self, visible: bool) {
            let visible = visible && self.frame.is_some_and(|f| !f.is_empty());
            if self.visible != visible {
                self.visible = visible;
                self.surface.set_visible(visible);
            }
        }

        pub fn is_visible(&self) -> bool {
            self.visible
        }
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "windows"
))]
pub use surface::VideoSurface;

/// Pins the floating controls window over a rectangle of the player window.
///
/// The controls are a separate GPUI window (GPUI cannot draw over the native video surface);
/// this ties it to the player window natively: an X11 child window, an AppKit child window, or
/// a Win32 owned window. See the platform files for details.
#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "windows"
))]
pub struct OverlayHost {
    host: overlay_platform::Host,
    frame: Option<(Frame, f32)>,
    visible: Option<bool>,
}

#[cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "windows"
))]
impl OverlayHost {
    pub fn attach(
        main: raw_window_handle::RawWindowHandle,
        overlay: raw_window_handle::RawWindowHandle,
    ) -> Result<Self, String> {
        Ok(Self {
            host: overlay_platform::Host::attach(main, overlay)?,
            frame: None,
            visible: None,
        })
    }

    /// Place the controls; `frame` is in the player window's client coordinates. No-op when
    /// unchanged.
    pub fn set_frame(&mut self, frame: Frame, corner_radius: f32) {
        if self.frame != Some((frame, corner_radius)) {
            self.frame = Some((frame, corner_radius));
            self.host.set_frame(frame, corner_radius);
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        let visible = visible && self.frame.is_some();
        if self.visible != Some(visible) {
            self.visible = Some(visible);
            self.host.set_visible(visible);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Frame;

    #[test]
    fn frame_rounds_outward_at_fractional_scale() {
        let frame = Frame {
            x: 10.3,
            y: 0.0,
            width: 100.2,
            height: 50.0,
            scale: 1.5,
        };
        // 15.45 → 15, right edge 165.75 → 166.
        assert_eq!(frame.to_device(), (15, 0, 151, 75));
        assert!(!frame.is_empty());
        assert!(
            Frame {
                width: 0.5,
                ..frame
            }
            .is_empty()
        );
    }
}
