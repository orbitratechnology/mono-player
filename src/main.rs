//! Mono Player entry point.

use std::path::{Path, PathBuf};

fn main() {
    configure_bundled_runtime();
    let paths: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .filter(|arg| !arg.to_string_lossy().starts_with("--"))
        .map(PathBuf::from)
        .collect();
    mono_player::app::run(paths);
}

/// Point LibVLC at the plugins shipped next to the executable, so installed builds never depend
/// on a system VLC. A developer build without a bundle falls back to the system LibVLC.
fn configure_bundled_runtime() {
    if std::env::var_os("VLC_PLUGIN_PATH").is_some() {
        return;
    }
    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return;
    };
    let candidates = [
        exe_dir.join("plugins"),                    // Windows / portable bundle
        exe_dir.join("../lib/mono-player/plugins"), // Linux package
        exe_dir.join("../Frameworks/plugins"),      // macOS .app bundle
    ];
    if let Some(dir) = candidates.iter().find(|dir| dir.is_dir()) {
        // SAFETY: called first thing in `main`, before any other thread exists.
        unsafe { std::env::set_var("VLC_PLUGIN_PATH", dir) };
    }
}
