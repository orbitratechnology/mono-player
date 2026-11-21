//! A LibVLC media player plus its current media, with ordered teardown.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::path::Path;

use vlc::sys;
use vlc::{MediaPlayerAudioEx as _, MediaPlayerVideoEx as _, Meta, Position};

use super::events::{EngineSink, EventSubscription, MEDIA_EVENTS, PLAYER_EVENTS};
use super::instance::Runtime;
use crate::playback::PlaybackError;
use crate::playback::features::{
    AudioDevice, ChapterInfo, EqualizerInfo, EqualizerPreset, EqualizerSettings, MediaMeta,
    Navigation, StereoMode, TitleInfo, VideoAdjust, file_uri_to_path,
};
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

    // LibVLC >= 3.0 title/chapter descriptions with durations and offsets.
    fn libvlc_media_player_get_full_title_descriptions(
        p_mi: *mut sys::libvlc_media_player_t,
        titles: *mut *mut *mut TitleDescription,
    ) -> c_int;
    fn libvlc_title_descriptions_release(titles: *mut *mut TitleDescription, count: c_uint);
    fn libvlc_media_player_get_full_chapter_descriptions(
        p_mi: *mut sys::libvlc_media_player_t,
        title: c_int,
        chapters: *mut *mut *mut ChapterDescription,
    ) -> c_int;
    fn libvlc_chapter_descriptions_release(chapters: *mut *mut ChapterDescription, count: c_uint);
    // LibVLC >= 3.0: current output device id (free with libvlc_free).
    fn libvlc_audio_output_device_get(p_mi: *mut sys::libvlc_media_player_t) -> *mut c_char;
}

/// `libvlc_title_description_t` (LibVLC 3.0).
#[repr(C)]
struct TitleDescription {
    duration: i64,
    name: *mut c_char,
    flags: c_uint,
}

/// `libvlc_chapter_description_t` (LibVLC 3.0).
#[repr(C)]
struct ChapterDescription {
    time_offset: i64,
    duration: i64,
    name: *mut c_char,
}

const TITLE_MENU_FLAG: c_uint = 0x01;

// `libvlc_video_adjust_option_t`.
const ADJUST_ENABLE: c_uint = 0;
const ADJUST_CONTRAST: c_uint = 1;
const ADJUST_BRIGHTNESS: c_uint = 2;
const ADJUST_HUE: c_uint = 3;
const ADJUST_SATURATION: c_uint = 4;
const ADJUST_GAMMA: c_uint = 5;

/// The player and its media. `!Send`: lives on the playback thread.
///
/// Field order is the teardown order (Rust drops fields top to bottom): callbacks are detached
/// first, then the media is released, then the player.
pub struct Player {
    events: Option<EventSubscription>,
    media_events: Option<EventSubscription>,
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
            media_events: None,
            media: None,
            player,
        })
    }

    /// Stop, detach the current media's callbacks and release it. After this returns no callback
    /// for the previous media can run.
    pub fn unload(&mut self) {
        self.player.stop();
        self.events.take();
        self.media_events.take();
        self.media.take();
    }

    /// Replace the current media and subscribe `sink` to its events.
    pub fn load(
        &mut self,
        runtime: &Runtime,
        source: &MediaSource,
        start_ms: Option<i64>,
        options: &[String],
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

        let start = start_ms
            .filter(|ms| *ms > 0)
            .map(|ms| format!(":start-time={:.3}", ms as f64 / 1000.0));
        for option in options.iter().chain(start.as_ref()) {
            let Ok(option) = CString::new(option.as_str()) else {
                continue;
            };
            // SAFETY: valid media pointer and NUL-terminated option string.
            unsafe { sys::libvlc_media_add_option(media.raw(), option.as_ptr()) };
        }

        self.player.set_media(&media);
        // SAFETY: the event managers belong to `self.player` and `media`, which outlive the
        // subscriptions (dropped first in `unload` and `Drop`, per field order).
        unsafe {
            self.events = Some(EventSubscription::attach(
                self.player.event_manager().raw(),
                PLAYER_EVENTS,
                sink.clone(),
            ));
            self.media_events = Some(EventSubscription::attach(
                sys::libvlc_media_event_manager(media.raw()),
                MEDIA_EVENTS,
                sink,
            ));
        }
        self.media = Some(media);
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
                video: walk_descriptions(sys::libvlc_video_get_track_description(raw)),
                current_audio: Some(sys::libvlc_audio_get_track(raw)).filter(|id| *id >= 0),
                current_subtitle: Some(sys::libvlc_video_get_spu(raw)),
                current_video: Some(sys::libvlc_video_get_track(raw)).filter(|id| *id >= 0),
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

    pub fn select_video_track(&self, id: i32) -> bool {
        // SAFETY: valid player pointer.
        unsafe { sys::libvlc_video_set_track(self.player.raw(), id) == 0 }
    }

    /// The equalizer LibVLC ships: band frequencies and presets. Needs no player.
    pub fn equalizer_info() -> EqualizerInfo {
        // SAFETY: static LibVLC queries; every equalizer created here is released.
        unsafe {
            let bands = sys::libvlc_audio_equalizer_get_band_count();
            let frequencies = (0..bands)
                .map(|i| sys::libvlc_audio_equalizer_get_band_frequency(i))
                .collect();
            let presets = (0..sys::libvlc_audio_equalizer_get_preset_count())
                .filter_map(|index| {
                    let name = sys::libvlc_audio_equalizer_get_preset_name(index);
                    let eq = sys::libvlc_audio_equalizer_new_from_preset(index);
                    if name.is_null() || eq.is_null() {
                        return None;
                    }
                    let preset = EqualizerPreset {
                        name: CStr::from_ptr(name).to_string_lossy().into_owned(),
                        preamp: sys::libvlc_audio_equalizer_get_preamp(eq),
                        bands: (0..bands)
                            .map(|band| sys::libvlc_audio_equalizer_get_amp_at_index(eq, band))
                            .collect(),
                    };
                    sys::libvlc_audio_equalizer_release(eq);
                    Some(preset)
                })
                .collect();
            EqualizerInfo {
                frequencies,
                presets,
            }
        }
    }

    /// Apply (or with `None`, remove) the equalizer. LibVLC copies the settings.
    pub fn set_equalizer(&self, settings: Option<&EqualizerSettings>) -> bool {
        let raw = self.player.raw();
        // SAFETY: valid player; the temporary equalizer is released after LibVLC copied it.
        unsafe {
            let Some(settings) = settings.filter(|s| s.enabled) else {
                return sys::libvlc_media_player_set_equalizer(raw, std::ptr::null_mut()) == 0;
            };
            let eq = sys::libvlc_audio_equalizer_new();
            if eq.is_null() {
                return false;
            }
            sys::libvlc_audio_equalizer_set_preamp(eq, settings.preamp);
            for (band, amp) in settings.bands.iter().enumerate() {
                sys::libvlc_audio_equalizer_set_amp_at_index(eq, *amp, band as c_uint);
            }
            let ok = sys::libvlc_media_player_set_equalizer(raw, eq) == 0;
            sys::libvlc_audio_equalizer_release(eq);
            ok
        }
    }

    /// Picture adjustments; neutral values switch the adjust filter off.
    pub fn set_video_adjust(&self, adjust: &VideoAdjust) {
        let raw = self.player.raw();
        // SAFETY: valid player pointer; option ids from `libvlc_video_adjust_option_t`.
        unsafe {
            if adjust.is_neutral() {
                sys::libvlc_video_set_adjust_int(raw, ADJUST_ENABLE, 0);
                return;
            }
            sys::libvlc_video_set_adjust_int(raw, ADJUST_ENABLE, 1);
            sys::libvlc_video_set_adjust_float(raw, ADJUST_CONTRAST, adjust.contrast);
            sys::libvlc_video_set_adjust_float(raw, ADJUST_BRIGHTNESS, adjust.brightness);
            sys::libvlc_video_set_adjust_float(raw, ADJUST_HUE, adjust.hue);
            sys::libvlc_video_set_adjust_float(raw, ADJUST_SATURATION, adjust.saturation);
            sys::libvlc_video_set_adjust_float(raw, ADJUST_GAMMA, adjust.gamma);
        }
    }

    pub fn set_aspect_ratio(&self, ratio: Option<&str>) {
        self.player.set_aspect_ratio(ratio);
    }

    pub fn set_crop(&self, geometry: Option<&str>) {
        let geometry = geometry.and_then(|g| CString::new(g).ok());
        // SAFETY: valid player; null clears the crop.
        unsafe {
            sys::libvlc_video_set_crop_geometry(
                self.player.raw(),
                geometry.as_ref().map_or(std::ptr::null(), |g| g.as_ptr()),
            );
        }
    }

    /// 0 fits the video to the surface.
    pub fn set_zoom(&self, factor: f32) {
        self.player.set_scale(factor.max(0.0));
    }

    pub fn set_deinterlace(&self, mode: Option<&str>) {
        let mode = mode.and_then(|m| CString::new(m).ok());
        // SAFETY: valid player; null disables deinterlacing.
        unsafe {
            sys::libvlc_video_set_deinterlace(
                self.player.raw(),
                mode.as_ref().map_or(std::ptr::null(), |m| m.as_ptr()),
            );
        }
    }

    /// Ask LibVLC to save the current frame (original size) into `dir`. Completion arrives as
    /// `EngineEvent::SnapshotTaken`.
    pub fn take_snapshot(&self, dir: &Path) -> bool {
        let mut path = dir.to_string_lossy().into_owned();
        if !path.ends_with(std::path::MAIN_SEPARATOR) {
            path.push(std::path::MAIN_SEPARATOR);
        }
        let Ok(path) = CString::new(path) else {
            return false;
        };
        // SAFETY: valid player and NUL-terminated directory path.
        unsafe { sys::libvlc_video_take_snapshot(self.player.raw(), 0, path.as_ptr(), 0, 0) == 0 }
    }

    /// Advance one frame (pauses playback).
    pub fn next_frame(&self) {
        self.player.next_frame();
    }

    pub fn set_title(&self, index: i32) {
        self.player.set_title(index);
    }

    pub fn set_chapter(&self, index: i32) {
        self.player.set_chapter(index);
    }

    /// Titles and chapters of the current media.
    pub fn navigation(&self) -> Navigation {
        let raw = self.player.raw();
        let mut navigation = Navigation::default();
        // SAFETY: valid player; arrays returned by LibVLC are walked within their reported
        // length and released exactly once.
        unsafe {
            let mut titles: *mut *mut TitleDescription = std::ptr::null_mut();
            let count = libvlc_media_player_get_full_title_descriptions(raw, &mut titles);
            if count > 0 && !titles.is_null() {
                for index in 0..count {
                    let title = &**titles.add(index as usize);
                    navigation.titles.push(TitleInfo {
                        index,
                        name: c_string_or(title.name, || format!("Title {}", index + 1)),
                        duration_ms: title.duration,
                        is_menu: title.flags & TITLE_MENU_FLAG != 0,
                    });
                }
                libvlc_title_descriptions_release(titles, count as c_uint);
            }
            let mut chapters: *mut *mut ChapterDescription = std::ptr::null_mut();
            let count = libvlc_media_player_get_full_chapter_descriptions(raw, -1, &mut chapters);
            if count > 0 && !chapters.is_null() {
                for index in 0..count {
                    let chapter = &**chapters.add(index as usize);
                    navigation.chapters.push(ChapterInfo {
                        index,
                        name: c_string_or(chapter.name, || format!("Chapter {}", index + 1)),
                        offset_ms: chapter.time_offset,
                        duration_ms: chapter.duration,
                    });
                }
                libvlc_chapter_descriptions_release(chapters, count as c_uint);
            }
        }
        navigation.current_title = self.player.get_title().filter(|t| *t >= 0);
        navigation.current_chapter = self.player.get_chapter().filter(|c| *c >= 0);
        navigation
    }

    /// Devices of the current audio output module, and the selected one.
    pub fn audio_devices(&self) -> (Vec<AudioDevice>, Option<String>) {
        let raw = self.player.raw();
        let mut devices = Vec::new();
        // SAFETY: valid player; the list is walked and released once; the current id string is
        // freed with libvlc_free.
        unsafe {
            let head = sys::libvlc_audio_output_device_enum(raw);
            let mut node = head;
            while !node.is_null() {
                let entry = &*node;
                if !entry.psz_device.is_null() {
                    let id = CStr::from_ptr(entry.psz_device)
                        .to_string_lossy()
                        .into_owned();
                    let name = c_string_or(entry.psz_description, || id.clone());
                    devices.push(AudioDevice { id, name });
                }
                node = entry.p_next;
            }
            if !head.is_null() {
                sys::libvlc_audio_output_device_list_release(head);
            }
            let current = libvlc_audio_output_device_get(raw);
            let current_id = (!current.is_null()).then(|| {
                let id = CStr::from_ptr(current).to_string_lossy().into_owned();
                sys::libvlc_free(current.cast::<c_void>());
                id
            });
            (devices, current_id.filter(|id| !id.is_empty()))
        }
    }

    /// Switch output device (`None`: the system default).
    pub fn set_audio_device(&self, id: Option<&str>) {
        let id = CString::new(id.unwrap_or("")).unwrap_or_default();
        // SAFETY: valid player; null module keeps the current output module.
        unsafe {
            sys::libvlc_audio_output_device_set(self.player.raw(), std::ptr::null(), id.as_ptr())
        };
    }

    pub fn set_stereo_mode(&self, mode: StereoMode) -> bool {
        // SAFETY: valid player pointer.
        unsafe { sys::libvlc_audio_set_channel(self.player.raw(), mode.to_libvlc()) == 0 }
    }

    /// Metadata of the current media (title, artist, album, now playing, local artwork).
    pub fn meta(&self) -> MediaMeta {
        let Some(media) = &self.media else {
            return MediaMeta::default();
        };
        let get = |meta: Meta| media.get_meta(meta).filter(|v| !v.trim().is_empty());
        MediaMeta {
            title: get(Meta::Title),
            artist: get(Meta::Artist),
            album: get(Meta::Album),
            genre: get(Meta::Genre),
            date: get(Meta::Date),
            now_playing: get(Meta::NowPlaying),
            artwork: get(Meta::ArtworkURL).and_then(|url| file_uri_to_path(&url)),
        }
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

/// A LibVLC C string, or `fallback()` when it is null or empty.
///
/// # Safety
/// `ptr` must be null or a valid NUL-terminated string.
unsafe fn c_string_or(ptr: *const c_char, fallback: impl FnOnce() -> String) -> String {
    if ptr.is_null() {
        return fallback();
    }
    // SAFETY: per the caller's contract.
    let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
    if text.trim().is_empty() {
        fallback()
    } else {
        text.into_owned()
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
