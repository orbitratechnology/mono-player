//! The LibVLC runtime (`libvlc_instance_t`), created lazily on the playback thread.

use std::ffi::CStr;

use vlc::sys;

use crate::playback::PlaybackError;

/// Owns the LibVLC instance. `!Send`: created, used and dropped on the playback thread only.
pub struct Runtime {
    instance: vlc::Instance,
}

impl Runtime {
    /// Initialize LibVLC and load its plugins. Slow on first call (plugin cache scan), which is
    /// why it runs on the playback thread and only when media is first requested.
    pub fn start() -> Result<Self, PlaybackError> {
        let instance = vlc::Instance::new().ok_or(PlaybackError::RuntimeUnavailable)?;
        let version = env!("CARGO_PKG_VERSION");
        instance.set_user_agent("Mono Player", &format!("MonoPlayer/{version}"));
        instance.set_app_id("app.monoplayer.MonoPlayer", version, "");
        Ok(Self { instance })
    }

    /// The LibVLC version string, e.g. `3.0.20 Vetinari`.
    pub fn version() -> String {
        // SAFETY: returns a pointer to a static NUL-terminated string (or null).
        let ptr = unsafe { sys::libvlc_get_version() };
        if ptr.is_null() {
            return String::from("unknown");
        }
        // SAFETY: non-null, static, NUL-terminated per the LibVLC API contract.
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }

    pub(super) fn instance(&self) -> &vlc::Instance {
        &self.instance
    }
}
