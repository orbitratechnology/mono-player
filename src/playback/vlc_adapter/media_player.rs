//! A LibVLC media player plus its current media, with ordered teardown.

use std::ffi::{CStr, CString, c_char};
use std::path::Path;

use vlc::sys;
use vlc::{MediaPlayerAudioEx as _, MediaPlayerVideoEx as _, Position};

use super::events::{EngineSink, EventSubscription};
use super::instance::Runtime;
use crate::playback::PlaybackError;
use crate::playback::protocol::{MediaSource, path_to_file_uri};
use crate::playback::state::{TrackInfo, Tracks};
use crate::video_output::VideoTarget;

// LibVLC 3.0 `libvlc_media_slave_type_subtitle`.
const SLAVE_TYPE_SUBTITLE: std::ffi::c_int = 0;

unsafe extern "C" {
    // Not exposed by vlc-rs 0.3; LibVLC >= 3.0.
    fn libvlc_media_player_add_slave(
        p_mi: *mut sys::libvlc_media_player_t,
        i_type: std::ffi::c_int,
        psz_uri: *const c_char,
        b_select: bool,
    ) -> std::ffi::c_int;
}

/// The player and its media. `!Send`: lives on the playback thread.
///
/// Field order is the teardown order (Rust drops fields top to bottom): callbacks are detached
/// first, then the media is released, then the player.
pub struct Player {
    events: Option<EventSubscription>,
    media: Option<vlc::Media>,
    player: vlc::MediaPlayer,
}

impl Player {
    pub fn new(runtime: &Runtime) -> Result<Self, PlaybackError> {
        let player =
            vlc::MediaPlayer::new(runtime.instance()).ok_or(PlaybackError::PlayerUnavailable)?;
        // The native surface must stay transparent to input so the GPUI window keeps receiving
        // pointer and keyboard events over the video.
        player.set_key_input(false);
        player.set_mouse_input(false);
        // No filename overlay on the picture: the UI already shows the title.
        player.set_video_title_display(Position::Disable, 0);
        Ok(Self {
            events: None,
            media: None,
            player,
        })
    }

    /// Stop, detach the current media's callbacks and release it. After this returns no callback
    /// for the previous media can run.
    pub fn unload(&mut self) {
        self.player.stop();
        self.events.take();
        self.media.take();
    }

    /// Replace the current media and subscribe `sink` to its events.
    pub fn load(
        &mut self,
        runtime: &Runtime,
        source: &MediaSource,
        start_ms: Option<i64>,
        sink: EngineSink,
    ) -> Result<(), PlaybackError> {
        self.unload();
        let name = source.display_name();
        let media = match source {
            // On Unix a percent-encoded URI keeps non-UTF-8 filenames intact.
            MediaSource::File(path) if cfg!(unix) => {
                vlc::Media::new_location(runtime.instance(), &path_to_file_uri(path))
            }
            MediaSource::File(path) => vlc::Media::new_path(runtime.instance(), path),
            // Validated URLs contain no NUL (vlc-rs panics on interior NULs).
            MediaSource::Url(url) => vlc::Media::new_location(runtime.instance(), url),
        }
        .ok_or(PlaybackError::OpenFailed { name })?;

        if let Some(start) = start_ms.filter(|ms| *ms > 0) {
            let option = CString::new(format!(":start-time={:.3}", start as f64 / 1000.0))
                .expect("formatted number has no NUL");
            // SAFETY: valid media pointer and NUL-terminated option string.
            unsafe { sys::libvlc_media_add_option(media.raw(), option.as_ptr()) };
        }

        self.player.set_media(&media);
        self.media = Some(media);
        // SAFETY: the event manager belongs to `self.player`, which outlives the subscription
        // (it is dropped in `unload` or first in `Drop`).
        self.events =
            Some(unsafe { EventSubscription::attach(self.player.event_manager().raw(), sink) });
        Ok(())
    }

    pub fn play(&self) -> bool {
        self.player.play().is_ok()
    }

    pub fn set_pause(&self, pause: bool) {
        self.player.set_pause(pause);
    }

    /// Synchronous in LibVLC 3: waits for the input thread. Only ever called on the playback
    /// thread, so the UI is never blocked by it.
    pub fn stop(&self) {
        self.player.stop();
    }

    pub fn time_ms(&self) -> Option<i64> {
        self.player.get_time()
    }

    pub fn length_ms(&self) -> Option<i64> {
        // SAFETY: valid player pointer.
        let length = unsafe { sys::libvlc_media_player_get_length(self.player.raw()) };
        (length > 0).then_some(length)
    }

    pub fn set_time(&self, ms: i64) {
        self.player.set_time(ms);
    }

    pub fn is_seekable(&self) -> bool {
        self.player.is_seekable()
    }

    pub fn has_video(&self) -> bool {
        self.player.has_vout() > 0
    }

    pub fn set_volume(&self, percent: u8) -> bool {
        self.player.set_volume(i32::from(percent)).is_ok()
    }

    pub fn set_muted(&self, muted: bool) {
        self.player.set_mute(muted);
    }

    pub fn set_rate(&self, rate: f32) -> bool {
        self.player.set_rate(rate).is_ok()
    }

    pub fn tracks(&self) -> Tracks {
        let raw = self.player.raw();
        // SAFETY: valid player pointer; the description lists are walked and released below.
        unsafe {
            Tracks {
                audio: walk_descriptions(sys::libvlc_audio_get_track_description(raw)),
                subtitles: walk_descriptions(sys::libvlc_video_get_spu_description(raw)),
                current_audio: Some(sys::libvlc_audio_get_track(raw)).filter(|id| *id >= 0),
                current_subtitle: Some(sys::libvlc_video_get_spu(raw)),
            }
        }
    }

    pub fn select_audio_track(&self, id: i32) -> bool {
        // SAFETY: valid player pointer.
        unsafe { sys::libvlc_audio_set_track(self.player.raw(), id) == 0 }
    }

    pub fn select_subtitle_track(&self, id: i32) -> bool {
        // SAFETY: valid player pointer.
        unsafe { sys::libvlc_video_set_spu(self.player.raw(), id) == 0 }
    }

    /// Audio delay relative to video, in milliseconds.
    pub fn set_audio_delay(&self, ms: i64) -> bool {
        // SAFETY: valid player pointer; LibVLC takes microseconds.
        unsafe { sys::libvlc_audio_set_delay(self.player.raw(), ms * 1000) == 0 }
    }

    /// Subtitle delay relative to video, in milliseconds.
    pub fn set_subtitle_delay(&self, ms: i64) -> bool {
        // SAFETY: valid player pointer; LibVLC takes microseconds.
        unsafe { sys::libvlc_video_set_spu_delay(self.player.raw(), ms * 1000) == 0 }
    }

    /// Attach an external subtitle file and select it.
    pub fn add_subtitle_file(&self, path: &Path) -> bool {
        let Ok(uri) = CString::new(path_to_file_uri(path)) else {
            return false;
        };
        // SAFETY: valid player pointer and NUL-terminated URI.
        unsafe {
            libvlc_media_player_add_slave(
                self.player.raw(),
                SLAVE_TYPE_SUBTITLE,
                uri.as_ptr(),
                true,
            ) == 0
        }
    }

    /// Point LibVLC at a native drawable (or detach with `None`). Takes effect for the next video
    /// output; the drawable must stay valid until it is detached or the player is dropped.
    pub fn set_video_target(&self, target: Option<VideoTarget>) {
        let raw = self.player.raw();
        // SAFETY: valid player pointer; the drawable's lifetime is guaranteed by the UI, which
        // only destroys a surface after the actor has detached it (see `video_output`).
        unsafe {
            match target {
                Some(VideoTarget::X11Window(xid)) => sys::libvlc_media_player_set_xwindow(raw, xid),
                Some(VideoTarget::Hwnd(hwnd)) => {
                    sys::libvlc_media_player_set_hwnd(raw, hwnd as *mut std::ffi::c_void)
                }
                Some(VideoTarget::NsView(view)) => {
                    sys::libvlc_media_player_set_nsobject(raw, view as *mut std::ffi::c_void)
                }
                None => {
                    sys::libvlc_media_player_set_xwindow(raw, 0);
                    sys::libvlc_media_player_set_hwnd(raw, std::ptr::null_mut());
                    sys::libvlc_media_player_set_nsobject(raw, std::ptr::null_mut());
                }
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Detach callbacks before the player and media go away.
        self.unload();
    }
}

/// Walk a full `libvlc_track_description_t` list (vlc-rs 0.3 skips the last entry) and free it.
///
/// # Safety
/// `head` must be null or a list returned by LibVLC that has not been released.
unsafe fn walk_descriptions(head: *mut sys::libvlc_track_description_t) -> Vec<TrackInfo> {
    let mut tracks = Vec::new();
    let mut node = head;
    while !node.is_null() {
        // SAFETY: `node` is a valid list element until the list is released below.
        let (id, name, next) = unsafe {
            let entry = &*node;
            let name = if entry.psz_name.is_null() {
                format!("Track {}", entry.i_id)
            } else {
                CStr::from_ptr(entry.psz_name)
                    .to_string_lossy()
                    .into_owned()
            };
            (entry.i_id, name, entry.p_next)
        };
        tracks.push(TrackInfo { id, name });
        node = next;
    }
    if !head.is_null() {
        // SAFETY: releasing the list LibVLC allocated, exactly once.
        unsafe { sys::libvlc_track_description_list_release(head) };
    }
    tracks
}
