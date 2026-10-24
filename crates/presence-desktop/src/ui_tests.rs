//! Regressions at the actual host/UI boundary, without network or real-time waits.
use super::*;
use slint::platform::{
    software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    Platform, PlatformError, WindowAdapter,
};
use std::cell::Cell;

struct TestPlatform {
    window: Rc<MinimalSoftwareWindow>,
    elapsed: Rc<Cell<Duration>>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.elapsed.get()
    }
}

#[test]
fn saved_card_artwork_loads_independently_of_draft_and_notifications_expire() {
    let elapsed = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(TestPlatform {
        window: MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer),
        elapsed: elapsed.clone(),
    }))
    .unwrap();
    let ui = MainWindow::new().unwrap();
    // This executor never runs tasks, so HTTPS requests remain queued locally.
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (sender, _commands) = unbounded_channel();
    let (updates, _receiver) = mpsc::channel();
    let mut config = Config::default();
    let first = "https://example.invalid/first.png";
    let second = "https://example.invalid/second.png";
    config.profiles[0].large_image = first.into();
    config.profiles[0].small_image = first.into();
    config.profiles[1].large_image = second.into();
    config.profiles[2].large_image = "uploaded_asset".into();
    let mut host = Host {
        editor: Editor::new(config),
        runtime: RuntimeState::default(),
        sender,
        updates,
        executor: executor.handle().clone(),
        pending: None,
        rule: None,
        resume_after_save: false,
        shutting_down: false,
        artwork: HashMap::new(),
        artwork_requests: HashMap::new(),
        artwork_retry_after: HashMap::new(),
        notification_timer: slint::Timer::default(),
    };
    host.refresh(&ui);
    assert_eq!(
        host.artwork_requests.len(),
        2,
        "shared URLs load once, including unselected cards"
    );
    for (url, color) in [(first, [255, 180, 30, 255]), (second, [30, 180, 255, 255])] {
        let pixels = artwork::Pixels {
            width: 8,
            height: 8,
            rgba: color.repeat(64),
        }
        .rounded();
        host.receive_artwork(url.into(), Ok(pixels));
    }
    host.refresh(&ui);
    let rows = ui.get_profiles();
    assert!(rows.row_data(0).unwrap().has_artwork);
    assert!(
        rows.row_data(1).unwrap().has_artwork,
        "unselected card gets its own image"
    );
    assert!(
        !rows.row_data(2).unwrap().has_artwork,
        "asset keys retain the theme fallback"
    );
    assert_ne!(
        rows.row_data(0).unwrap().artwork,
        rows.row_data(1).unwrap().artwork
    );
    assert_eq!(ui.get_preview_large().size(), (186, 186).into());
    assert_eq!(ui.get_editor_artwork().size(), (264, 264).into());
    assert_eq!(ui.get_preview_small().size(), (57, 57).into());
    host.editor.draft.large_image = second.into();
    host.refresh(&ui);
    assert_eq!(ui.get_preview_large(), host.artwork[second].preview);
    assert_eq!(
        ui.get_profiles().row_data(0).unwrap().artwork,
        host.artwork[first].card,
        "unsaved artwork does not change the saved card"
    );
    host.editor.draft.large_image = "https://example.invalid/failed.png".into();
    host.refresh(&ui);
    assert!(
        !ui.get_has_preview_large(),
        "changing a URL clears the previous preview"
    );
    assert_eq!(ui.get_preview_large().size().width, 0);
    host.receive_artwork(
        host.editor.draft.large_image.clone(),
        Err("unavailable".into()),
    );
    host.refresh(&ui);
    assert!(
        host.artwork_requests.is_empty(),
        "failed artwork respects retry backoff"
    );

    let advance = |seconds| {
        elapsed.set(Duration::from_secs(seconds));
        slint::platform::update_timers_and_animations();
    };
    notify(&ui, &host.notification_timer, "Presence stopped.");
    advance(4);
    assert_eq!(ui.get_message(), "Presence stopped.");
    // Repeating the same message must also start a fresh five-second interval.
    notify(&ui, &host.notification_timer, "Presence stopped.");
    advance(5);
    assert_eq!(ui.get_message(), "Presence stopped.");
    advance(9);
    assert!(ui.get_message().is_empty());
    notify(&ui, &host.notification_timer, "Saved.");
    advance(12);
    notify(&ui, &host.notification_timer, "New notice.");
    advance(14);
    assert_eq!(
        ui.get_message(),
        "New notice.",
        "an older timeout cannot dismiss a newer notice"
    );
    host.action(&ui, "dismiss-message", "");
    assert!(
        ui.get_message().is_empty(),
        "the close button still dismisses immediately"
    );
    advance(18);
    assert!(ui.get_message().is_empty());
}
