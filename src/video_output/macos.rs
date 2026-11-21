//! AppKit subview for LibVLC.
//!
//! The subview overrides `hitTest:` to return nil, so it and LibVLC's own video view inside it
//! are transparent to the mouse: clicks and movement reach the GPUI view underneath.

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSResponder, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::RawWindowHandle;

use super::types::{Frame, VideoTarget};

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "MonoPlayerVideoSurfaceView"]
    struct PassthroughView;

    impl PassthroughView {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }
    }
);

pub struct Surface {
    view: Retained<PassthroughView>,
}

impl Surface {
    pub fn create(parent: RawWindowHandle) -> Result<Self, String> {
        let RawWindowHandle::AppKit(handle) = parent else {
            return Err(format!("unexpected window handle {parent:?}"));
        };
        let mtm =
            MainThreadMarker::new().ok_or("video surface must be created on the main thread")?;
        // SAFETY: GPUI hands out its live content NSView; we only borrow it here.
        let parent: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
        // SAFETY: standard NSView designated initializer on a freshly allocated instance.
        let view: Retained<PassthroughView> =
            unsafe { msg_send![PassthroughView::alloc(mtm), initWithFrame: frame] };
        view.setHidden(true);
        parent.addSubview(&view);
        Ok(Self { view })
    }

    pub fn target(&self) -> VideoTarget {
        VideoTarget::NsView(Retained::as_ptr(&self.view) as usize)
    }

    pub fn set_frame(&mut self, frame: Frame) {
        // AppKit works in points (logical pixels); flip unless the parent is flipped.
        // SAFETY: reading the superview of our own live view on the main thread.
        let parent = unsafe { self.view.superview() };
        let y = match parent {
            Some(parent) if !parent.isFlipped() => {
                parent.frame().size.height - f64::from(frame.y) - f64::from(frame.height)
            }
            _ => f64::from(frame.y),
        };
        self.view.setFrame(NSRect::new(
            NSPoint::new(f64::from(frame.x), y),
            NSSize::new(
                f64::from(frame.width.max(1.0)),
                f64::from(frame.height.max(1.0)),
            ),
        ));
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.view.setHidden(!visible);
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.view.removeFromSuperview();
    }
}
