//! Minimal local diagnostics on stderr. No telemetry, no media contents; never panics.

use std::io::Write as _;
use std::sync::OnceLock;

fn verbose() -> bool {
    static VERBOSE: OnceLock<bool> = OnceLock::new();
    *VERBOSE.get_or_init(|| std::env::var_os("MONO_PLAYER_LOG").is_some())
}

/// Something went wrong but the app recovered.
pub fn warn(message: &str) {
    let _ = writeln!(std::io::stderr(), "[mono-player] warning: {message}");
}

/// Lifecycle tracing, enabled with `MONO_PLAYER_LOG=1`.
pub fn info(message: &str) {
    if verbose() {
        let _ = writeln!(std::io::stderr(), "[mono-player] {message}");
    }
}
