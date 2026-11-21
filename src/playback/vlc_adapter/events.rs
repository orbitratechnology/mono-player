//! LibVLC event subscription with deterministic detach.
//!
//! `vlc-rs` 0.3 can attach callbacks but never detaches them and leaks their closures, so this
//! module owns the attach/detach pair itself. Callbacks run on LibVLC's threads: they only
//! normalize the event and hand it to a non-blocking sink. They never touch GPUI, never block,
//! and never unwind across the FFI boundary.

use std::ffi::{CStr, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use vlc::sys;

use crate::diagnostics as log;

// LibVLC 3.0 `libvlc_event_e` values (verified against `vlc/libvlc_events.h`).
const OPENING: c_int = 0x102;
const BUFFERING: c_int = 0x103;
const PLAYING: c_int = 0x104;
const PAUSED: c_int = 0x105;
const STOPPED: c_int = 0x106;
const END_REACHED: c_int = 0x109;
const ENCOUNTERED_ERROR: c_int = 0x10A;
const TIME_CHANGED: c_int = 0x10B;
const SEEKABLE_CHANGED: c_int = 0x10D;
const TITLE_CHANGED: c_int = 0x10F;
const SNAPSHOT_TAKEN: c_int = 0x110;
const LENGTH_CHANGED: c_int = 0x111;
const VOUT: c_int = 0x112;
const ES_ADDED: c_int = 0x114;
const ES_DELETED: c_int = 0x115;
const ES_SELECTED: c_int = 0x116;
const AUDIO_DEVICE: c_int = 0x11C;
const CHAPTER_CHANGED: c_int = 0x11D;
// Media (not player) event.
const MEDIA_META_CHANGED: c_int = 0x000;

/// Events subscribed on the media player's event manager.
pub const PLAYER_EVENTS: &[c_int] = &[
    OPENING,
    BUFFERING,
    PLAYING,
    PAUSED,
    STOPPED,
    END_REACHED,
    ENCOUNTERED_ERROR,
    TIME_CHANGED,
    SEEKABLE_CHANGED,
    LENGTH_CHANGED,
    VOUT,
    ES_ADDED,
    ES_DELETED,
    ES_SELECTED,
    TITLE_CHANGED,
    SNAPSHOT_TAKEN,
    AUDIO_DEVICE,
    CHAPTER_CHANGED,
];

/// Events subscribed on the media's event manager.
pub const MEDIA_EVENTS: &[c_int] = &[MEDIA_META_CHANGED];

/// Minimum spacing of forwarded `TimeChanged` events. The actor applies the user-facing cadence;
/// this only bounds how much work reaches its queue.
const TIME_EVENT_FLOOR_MS: u64 = 50;

unsafe extern "C" {
    // Not exposed by vlc-rs 0.3. Present in every LibVLC since 1.0.
    fn libvlc_event_detach(
        p_event_manager: *mut sys::libvlc_event_manager_t,
        i_event_type: sys::libvlc_event_type_t,
        f_callback: sys::libvlc_callback_t,
        p_user_data: *mut c_void,
    );
}

/// A normalized LibVLC media-player event.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    Opening,
    Buffering(f32),
    Playing,
    Paused,
    Stopped,
    EndReached,
    EncounteredError,
    TimeChanged(i64),
    LengthChanged(i64),
    SeekableChanged(bool),
    /// Number of active video outputs.
    Vout(i32),
    /// An elementary stream was added, removed or selected.
    TracksChanged,
    /// The current title or chapter changed (DVD/Blu-ray titles, chapter marks).
    NavigationChanged,
    /// A snapshot was written to this file.
    SnapshotTaken(String),
    /// The audio output device changed.
    AudioDeviceChanged,
    /// Title/artist/now-playing/artwork metadata changed.
    MetaChanged,
}

/// Where callbacks deliver events. Must be cheap and non-blocking.
pub type EngineSink = std::sync::Arc<dyn Fn(EngineEvent) + Send + Sync + 'static>;

struct CallbackContext {
    sink: EngineSink,
    epoch: Instant,
    last_time_event_ms: AtomicU64,
}

/// Keeps callbacks attached to one LibVLC event manager. Dropping it detaches every callback
/// *before* freeing the context they reference.
pub struct EventSubscription {
    manager: *mut sys::libvlc_event_manager_t,
    context: *mut CallbackContext,
    attached: Vec<c_int>,
}

impl EventSubscription {
    /// Attach callbacks for `events` ([`PLAYER_EVENTS`] or [`MEDIA_EVENTS`]).
    ///
    /// # Safety
    /// `manager` must be the event manager of a live LibVLC object (player or media) that
    /// outlives the returned subscription (drop the subscription before releasing the object).
    pub unsafe fn attach(
        manager: *mut sys::libvlc_event_manager_t,
        events: &[c_int],
        sink: EngineSink,
    ) -> Self {
        let context = Box::into_raw(Box::new(CallbackContext {
            sink,
            epoch: Instant::now(),
            last_time_event_ms: AtomicU64::new(0),
        }));
        let mut attached = Vec::with_capacity(events.len());
        for &event_type in events {
            // SAFETY: `manager` is live per the caller's contract; `context` stays valid until
            // `Drop` detaches this exact (type, callback, context) triple.
            let result = unsafe {
                sys::libvlc_event_attach(manager, event_type, on_event, context.cast::<c_void>())
            };
            if result == 0 {
                attached.push(event_type);
            } else {
                log::warn(&format!("could not attach LibVLC event {event_type:#x}"));
            }
        }
        Self {
            manager,
            context,
            attached,
        }
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        for &event_type in &self.attached {
            // SAFETY: detaching the same triple that was attached. LibVLC guarantees the
            // callback is not running and will not run again once detach returns.
            unsafe {
                libvlc_event_detach(
                    self.manager,
                    event_type,
                    on_event,
                    self.context.cast::<c_void>(),
                );
            }
        }
        // SAFETY: created by `Box::into_raw` in `attach`; no callback can observe it any more.
        drop(unsafe { Box::from_raw(self.context) });
    }
}

unsafe extern "C" fn on_event(event: *const sys::libvlc_event_t, data: *mut c_void) {
    // Never unwind into C.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if event.is_null() || data.is_null() {
            return;
        }
        // SAFETY: `data` is the `CallbackContext` registered in `attach`, alive until detach.
        let context = unsafe { &*data.cast::<CallbackContext>() };
        // SAFETY: LibVLC passes a valid event for the duration of the callback.
        let event = unsafe { &*event };
        if let Some(normalized) = normalize(event, context) {
            (context.sink)(normalized);
        }
    }));
}

fn normalize(event: &sys::libvlc_event_t, context: &CallbackContext) -> Option<EngineEvent> {
    // SAFETY (all union reads below): LibVLC fills the union member that matches `_type`.
    Some(match event._type {
        OPENING => EngineEvent::Opening,
        BUFFERING => EngineEvent::Buffering(unsafe { event.u.media_player_buffering.new_cache }),
        PLAYING => EngineEvent::Playing,
        PAUSED => EngineEvent::Paused,
        STOPPED => EngineEvent::Stopped,
        END_REACHED => EngineEvent::EndReached,
        ENCOUNTERED_ERROR => EngineEvent::EncounteredError,
        TIME_CHANGED => {
            let now = context.epoch.elapsed().as_millis() as u64;
            let last = context.last_time_event_ms.load(Ordering::Relaxed);
            if last != 0 && now.saturating_sub(last) < TIME_EVENT_FLOOR_MS {
                return None;
            }
            context
                .last_time_event_ms
                .store(now.max(1), Ordering::Relaxed);
            EngineEvent::TimeChanged(unsafe { event.u.media_player_time_changed.new_time })
        }
        SEEKABLE_CHANGED => EngineEvent::SeekableChanged(unsafe {
            event.u.media_player_seekable_changed.new_seekable != 0
        }),
        LENGTH_CHANGED => {
            EngineEvent::LengthChanged(unsafe { event.u.media_player_length_changed.new_length })
        }
        VOUT => EngineEvent::Vout(unsafe { event.u.media_player_vout.new_count }),
        ES_ADDED | ES_DELETED | ES_SELECTED => EngineEvent::TracksChanged,
        TITLE_CHANGED | CHAPTER_CHANGED => EngineEvent::NavigationChanged,
        SNAPSHOT_TAKEN => {
            let name = unsafe { event.u.media_player_snapshot_taken.psz_filename };
            if name.is_null() {
                return None;
            }
            // SAFETY: non-null, NUL-terminated, valid for the callback's duration.
            let name = unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned();
            EngineEvent::SnapshotTaken(name)
        }
        AUDIO_DEVICE => EngineEvent::AudioDeviceChanged,
        MEDIA_META_CHANGED => EngineEvent::MetaChanged,
        _ => return None,
    })
}
