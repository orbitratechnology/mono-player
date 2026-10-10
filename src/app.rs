//! Application bootstrap: assets, theme, key bindings and the main window.

use std::borrow::Cow;
use std::path::PathBuf;

use gpui_kit::{
    AppContext as _, AssetSource, Bounds, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions, point, px, size,
};

use crate::components::AppIcons;
use crate::diagnostics as log;
use crate::features::player::view::PlayerView;
use crate::persistence::settings::{self, Settings};
use crate::{actions, theme};

/// GPUI Kit's default component icons plus the few extra icons this app uses.
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = AppIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(AppIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// Run the application with media paths from the command line / file association.
pub fn run(initial_paths: Vec<PathBuf>) {
    let settings_path = settings::settings_path();
    // A few hundred bytes, read once before the UI exists.
    let settings = settings_path
        .as_deref()
        .map(settings::load)
        .unwrap_or_default();

    let application = gpui_kit::application().with_assets(AppAssets);
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let application = prefer_x11(application);

    application.run(move |cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        actions::bind_keys(cx);
        // Global fallback when no player window has focus.
        cx.on_action(|_: &actions::Quit, cx| cx.quit());

        let bounds = window_bounds(&settings, cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Mono Player".into()),
                ..Default::default()
            }),
            app_id: Some("mono-player".into()),
            window_min_size: Some(size(px(640.), px(400.))),
            ..Default::default()
        };
        let opened = gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| PlayerView::new(settings, settings_path, initial_paths, window, cx))
        });
        match opened {
            Ok(_) => cx.activate(true),
            Err(error) => {
                log::warn(&format!("could not open the main window: {error}"));
                cx.quit();
            }
        }
    });
}

fn window_bounds(settings: &Settings, cx: &gpui_kit::App) -> Bounds<gpui_kit::Pixels> {
    match settings.window.filter(|w| w.is_plausible()) {
        Some(w) => Bounds::new(point(px(w.x), px(w.y)), size(px(w.width), px(w.height))),
        None => Bounds::centered(None, size(px(1120.), px(700.)), cx),
    }
}

/// LibVLC embeds video through X11 window ids, so on Linux the app runs on X11 — natively or
/// through XWayland — whenever an X display is available. Set `MONO_PLAYER_WAYLAND=1` to use
/// native Wayland anyway (LibVLC then shows video in its own window).
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn prefer_x11(application: gpui_kit::Application) -> gpui_kit::Application {
    use gpui_kit::{GraphicalEnvironment, WindowingRequest};

    let mut environment = GraphicalEnvironment::detect();
    let has_x11 = environment
        .x11_display
        .as_ref()
        .is_some_and(|d| !d.is_empty());
    if has_x11 && std::env::var_os("MONO_PLAYER_WAYLAND").is_none() {
        environment.wayland_display = None;
        application.with_windowing(WindowingRequest::Windowed(environment))
    } else {
        application
    }
}
