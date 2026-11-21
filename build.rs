//! Locate LibVLC for linking.
//!
//! Linux distributions install `libvlc.so` on the default library path, so nothing is needed
//! there. On macOS (and Windows) LibVLC ships inside VLC itself, so this script:
//!
//! 1. uses `VLC_LIB_DIR` if set (the folder containing `libvlc.dylib` / `libvlc.lib`), else
//! 2. on macOS, looks for `VLC.app` in `/Applications` or `~/Applications`,
//!
//! and adds it to the link search path. On macOS it also adds run-time search paths: the app
//! bundle's `Contents/Frameworks` (where `scripts/package-macos.sh` puts LibVLC) and, for
//! `cargo run` / `cargo test`, the folder LibVLC was found in. It exports the matching plugins
//! folder as `MONO_PLAYER_DEV_VLC_PLUGINS` so development runs find VLC's plugins too.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=VLC_LIB_DIR");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        // Installed app bundles carry LibVLC next to the executable.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    }

    let lib_dir = env::var_os("VLC_LIB_DIR")
        .map(PathBuf::from)
        .or_else(|| (target_os == "macos").then(find_vlc_app_lib).flatten());

    let Some(lib_dir) = lib_dir else {
        if target_os == "macos" {
            println!(
                "cargo:warning=LibVLC not found. Install VLC in /Applications \
                 (https://www.videolan.org/vlc/) or set VLC_LIB_DIR to the folder \
                 containing libvlc.dylib."
            );
        }
        return;
    };

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    if target_os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
        // VLC.app layout: Contents/MacOS/lib and Contents/MacOS/plugins.
        let plugins = lib_dir.with_file_name("plugins");
        if plugins.is_dir() {
            println!(
                "cargo:rustc-env=MONO_PLAYER_DEV_VLC_PLUGINS={}",
                plugins.display()
            );
        }
    }
}

fn find_vlc_app_lib() -> Option<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = env::var_os("HOME") {
        roots.push(Path::new(&home).join("Applications"));
    }
    roots
        .into_iter()
        .map(|root| root.join("VLC.app/Contents/MacOS/lib"))
        .find(|lib| lib.join("libvlc.dylib").exists())
}
