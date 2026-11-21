//! UI integration tests: the real player view in a headless GPUI Kit window, driven through
//! native pointer and keyboard dispatch.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt, TestWindowExt};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowHandle, WindowOptions, base::Root, px, size,
};
use mono_player::features::player::controller::PlaybackController;
use mono_player::features::player::overlay::open_controls;
use mono_player::features::player::view::{Panel, PlayerView};
use mono_player::persistence::settings::Settings;
use mono_player::playback::PlaybackState;
use mono_player::playback::{Command, Event, PlaybackError, RequestId};
use mono_player::{actions, theme};

/// The player window, its floating controls window, and the player view.
fn open_player(
    cx: &mut TestAppContext,
) -> (WindowHandle<Root>, AnyWindowHandle, Entity<PlayerView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        actions::bind_keys(cx);
        let (window, view) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1200.), px(760.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                // No playback thread: commands are recorded and events are injected by the
                // test, keeping GPUI's test scheduler deterministic.
                cx.new(|cx| {
                    PlayerView::with_controller(
                        PlaybackController::recording(),
                        Settings::default(),
                        None,
                        Vec::new(),
                        window,
                        cx,
                    )
                })
            },
        )
        .expect("open test window");
        let controls = open_controls(window, view.clone(), cx).expect("open controls window");
        (
            window.downcast::<Root>().expect("Base Root"),
            controls,
            view,
        )
    })
}

#[gpui_kit::test]
fn controls_have_labels_and_touch_sized_hit_areas(cx: &mut TestAppContext) {
    let (handle, controls, _) = open_player(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // The empty state offers the primary actions.
        assert!(window.find("empty-open").visible());
        assert!(window.find("empty-address").visible());
        // The transport is not drawn in the player window: it floats in its own window.
        assert!(window.try_find("transport-play-pause").is_none());
    })
    .unwrap();
    cx.update_window(controls, |_, window, cx| {
        window.render_frame(cx);

        let play = window.find("transport-play-pause");
        assert_eq!(play.label(), Some("Play"));
        assert!(play.bounds().size.width >= px(48.) && play.bounds().size.height >= px(48.));

        for (id, label) in [
            ("transport-previous", "Previous"),
            ("transport-next", "Next"),
            ("volume-mute", "Mute"),
            ("panel-tracks", "Audio, subtitles & chapters"),
            ("panel-effects", "Equalizer & picture"),
            ("panel-queue", "Playlist"),
            ("panel-settings", "Settings"),
            ("toggle-fullscreen", "Fullscreen"),
        ] {
            let control = window.find(id);
            assert_eq!(control.label(), Some(label), "{id}");
            let bounds = control.bounds().size;
            assert!(
                bounds.width >= px(44.) && bounds.height >= px(44.),
                "{id} hit area is {bounds:?}"
            );
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn floating_controls_drive_the_player_window(cx: &mut TestAppContext) {
    let (_, controls, view) = open_player(cx);
    cx.update_window(controls, |_, window, cx| {
        window.render_frame(cx);
        // A click in the controls window runs in the player window.
        window.click("panel-settings", cx);
        assert_eq!(view.read(cx).panel(), Some(Panel::Settings));
        window.click("panel-effects", cx);
        assert_eq!(view.read(cx).panel(), Some(Panel::Effects));
        window.click("volume-mute", cx);
        assert!(view.read(cx).model().muted);
        window.click("speed", cx);
        assert_eq!(view.read(cx).model().rate, 1.25);
        // Keys pressed while the controls window has focus act on the player too.
        window.press("m", cx);
        assert!(!view.read(cx).model().muted);
    })
    .unwrap();
}

#[gpui_kit::test]
async fn controls_hide_during_video_and_return_on_activity(cx: &mut TestAppContext) {
    let (handle, controls, view) = open_player(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |player, cx| {
            player.open_paths(vec![PathBuf::from("/m/film.mkv")], true, window, cx)
        });
    })
    .unwrap();
    let sender = cx.update(|cx| view.read(cx).controller().event_sender());
    for event in [
        Event::StateChanged {
            request: RequestId(1),
            state: PlaybackState::Playing,
        },
        Event::VideoOutputChanged {
            request: RequestId(1),
            has_video: true,
        },
    ] {
        sender.unbounded_send(event).unwrap();
    }
    cx.run_until_parked();
    assert!(cx.update(|cx| view.read(cx).controls_visible()));
    // The pointer leaves the controls (the test pointer starts on top of them).
    cx.update_window(controls, |_, window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                position: gpui_kit::point(px(-10.), px(-10.)),
                pressed_button: None,
                modifiers: Default::default(),
            }),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();

    // Untouched video playback: the controls get out of the way…
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    assert!(!cx.update(|cx| view.read(cx).controls_visible()));

    // …and come back with any key press.
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("up", cx);
    })
    .unwrap();
    assert!(cx.update(|cx| view.read(cx).controls_visible()));

    // Paused video keeps them up.
    sender
        .unbounded_send(Event::StateChanged {
            request: RequestId(1),
            state: PlaybackState::Paused,
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert!(cx.update(|cx| view.read(cx).controls_visible()));
}

#[gpui_kit::test]
fn keyboard_shortcuts_drive_the_player(cx: &mut TestAppContext) {
    let (handle, controls, view) = open_player(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let volume = view.read(cx).model().volume;

        window.press("up", cx);
        assert_eq!(view.read(cx).model().volume, volume + 5);
        window.press("down", cx);
        window.press("down", cx);
        assert_eq!(view.read(cx).model().volume, volume - 5);

        window.press("m", cx);
        assert!(view.read(cx).model().muted);
    })
    .unwrap();
    cx.update_window(controls, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("volume-mute").label(), Some("Unmute"));
    })
    .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("m", cx);
        assert!(!view.read(cx).model().muted);

        window.press("l", cx);
        assert_eq!(view.read(cx).panel(), Some(Panel::Queue));
        let tab = window.find(gpui_kit::ElementId::NamedInteger("panel-tab".into(), 0));
        assert_eq!(tab.label(), Some("Queue, selected"));
        window.press("secondary-e", cx);
        assert_eq!(view.read(cx).panel(), Some(Panel::Effects));
        window.press("t", cx);
        assert_eq!(view.read(cx).panel(), Some(Panel::Tracks));
        // Escape closes the panel when not fullscreen.
        window.press("escape", cx);
        assert_eq!(view.read(cx).panel(), None);

        window.press("]", cx);
        assert_eq!(view.read(cx).model().rate, 1.25);
        window.press("[", cx);
        window.press("[", cx);
        assert_eq!(view.read(cx).model().rate, 0.75);
    })
    .unwrap();
}

#[gpui_kit::test]
fn text_field_consumes_player_keys(cx: &mut TestAppContext) {
    let (handle, _, view) = open_player(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("l", cx);
        let input = view.read(cx).address_input().clone();
        input.update(cx, |input, cx| input.focus(window, cx));
        window.render_frame(cx);

        window.input("m l", cx);
        let player = view.read(cx);
        assert!(!player.model().muted, "M must type, not mute");
        assert_eq!(player.panel(), Some(Panel::Queue), "L must type, not close");
        assert_eq!(input.read(cx).value().as_ref(), "m l");
    })
    .unwrap();
}

#[gpui_kit::test]
fn invalid_addresses_are_rejected_before_the_engine(cx: &mut TestAppContext) {
    let (handle, controls, view) = open_player(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("l", cx);
        let input = view.read(cx).address_input().clone();
        input.update(cx, |input, cx| input.focus(window, cx));
        window.input("javascript://alert(1)", cx);
        window.click("address-add", cx);
        assert!(matches!(
            view.read(cx).model().error,
            Some(PlaybackError::UnsupportedScheme(_))
        ));
        assert!(view.read(cx).playlist().is_empty());
    })
    .unwrap();
    // The error shows (non-modally) in the floating controls and can be dismissed there.
    cx.update_window(controls, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("error-banner").visible());
        window.click("error-dismiss", cx);
        assert!(view.read(cx).model().error.is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn queue_rows_reorder_remove_and_report_failures(cx: &mut TestAppContext) {
    let (handle, controls, view) = open_player(cx);
    let missing: Vec<PathBuf> = ["a.mp3", "b.mp3", "c.mp3"]
        .iter()
        .map(|name| PathBuf::from(format!("/nonexistent/mono-player-ui/{name}")))
        .collect();
    cx.update_window(handle.into(), |_, window, cx| {
        view.update(cx, |player, cx| {
            player.open_paths(missing, false, window, cx)
        });
        window.press("l", cx);
        window.render_frame(cx);

        let ids: Vec<u64> = view
            .read(cx)
            .playlist()
            .items()
            .iter()
            .map(|item| item.id.0)
            .collect();
        assert_eq!(ids.len(), 3);
        // The first item is current and labelled as such by geometry and weight, not color.
        assert_eq!(
            view.read(cx).playlist().current_id().map(|id| id.0),
            Some(ids[0])
        );

        let down = gpui_kit::ElementId::NamedInteger("row-down".into(), ids[0]);
        window.click(down, cx);
        let order: Vec<u64> = view
            .read(cx)
            .playlist()
            .items()
            .iter()
            .map(|i| i.id.0)
            .collect();
        assert_eq!(order, vec![ids[1], ids[0], ids[2]]);

        let remove = gpui_kit::ElementId::NamedInteger("row-remove".into(), ids[2]);
        window.click(remove, cx);
        assert_eq!(view.read(cx).playlist().len(), 2);

        // Rows keep a touch-sized height.
        let row = window.find(gpui_kit::ElementId::NamedInteger(
            "media-row".into(),
            ids[1],
        ));
        assert!(row.bounds().size.height >= px(44.));

        // Activating a row makes it current.
        let play = gpui_kit::ElementId::NamedInteger("media-row-play".into(), ids[1]);
        assert_eq!(window.find(play.clone()).label(), Some("Play b.mp3"));
        window.click(play, cx);
        assert_eq!(
            view.read(cx).playlist().current_id().map(|id| id.0),
            Some(ids[1])
        );
    })
    .unwrap();

    // Opening queued the first item; playing row b opened a second request.
    let sender = cx.update(|cx| {
        let player = view.read(cx);
        let opens: Vec<_> = player
            .controller()
            .recorded()
            .iter()
            .filter_map(|c| match c {
                Command::Open { request, .. } => Some(*request),
                _ => None,
            })
            .collect();
        assert_eq!(opens, vec![RequestId(1), RequestId(2)]);
        player.controller().event_sender()
    });

    // A late failure for the replaced request is ignored; one for the current request shows a
    // non-modal banner and marks the row.
    sender
        .unbounded_send(Event::Error {
            request: Some(RequestId(1)),
            error: PlaybackError::FileNotFound("/nonexistent/mono-player-ui/a.mp3".into()),
        })
        .unwrap();
    sender
        .unbounded_send(Event::Error {
            request: Some(RequestId(2)),
            error: PlaybackError::FileNotFound("/nonexistent/mono-player-ui/b.mp3".into()),
        })
        .unwrap();
    let view_for_wait = view.clone();
    cx.wait_for(handle.into(), Duration::from_secs(1), move |_, cx| {
        matches!(
            &view_for_wait.read(cx).model().error,
            Some(PlaybackError::FileNotFound(path)) if path.ends_with("b.mp3")
        )
    })
    .await;
    cx.update(|cx| {
        let player = view.read(cx);
        let failed: Vec<_> = player
            .playlist()
            .items()
            .iter()
            .filter(|i| i.failed)
            .map(|i| i.title.clone())
            .collect();
        assert_eq!(failed, vec!["b.mp3".to_owned()]);
    });
    cx.update_window(controls, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("error-banner").visible());
    })
    .unwrap();
}
