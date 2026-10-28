//! Regressions at the actual host/UI boundary, without network or real-time waits.
use super::*;
use presence_core::model::{Recording, UsageSession};
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
fn appearance_follows_live_system_changes_and_explicit_modes_override_them() {
    use slint::language::ColorScheme;
    // Simulate the same context update used by the pinned Winit backend when
    // Windows sends ThemeChanged, without modifying the user's Windows settings.
    use slint::private_unstable_api::re_exports::WindowInner;

    slint::platform::set_platform(Box::new(TestPlatform {
        window: MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer),
        elapsed: Rc::new(Cell::new(Duration::ZERO)),
    }))
    .unwrap();
    let ui = MainWindow::new().unwrap();
    let context = WindowInner::from_pub(ui.window()).context().clone();
    context.set_color_scheme(ColorScheme::Light);
    assert_eq!(ui.get_appearance(), "dark");
    assert!(
        ui.get_dark_appearance(),
        "OLED dark is the default even on a light system"
    );

    ui.set_appearance("system".into());
    slint::platform::update_timers_and_animations();
    assert!(!ui.get_dark_appearance());
    context.set_color_scheme(ColorScheme::Dark);
    assert!(
        ui.get_dark_appearance(),
        "system changes apply without restarting"
    );
    context.set_color_scheme(ColorScheme::Light);
    assert!(!ui.get_dark_appearance());

    ui.set_appearance("light".into());
    slint::platform::update_timers_and_animations();
    context.set_color_scheme(ColorScheme::Dark);
    assert!(
        !ui.get_dark_appearance(),
        "explicit light ignores system changes"
    );
    ui.set_appearance("dark".into());
    slint::platform::update_timers_and_animations();
    context.set_color_scheme(ColorScheme::Light);
    assert!(
        ui.get_dark_appearance(),
        "explicit dark ignores system changes"
    );
}

#[test]
fn appearance_save_preserves_drafts_and_applies_only_after_persistence_succeeds() {
    slint::platform::set_platform(Box::new(TestPlatform {
        window: MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer),
        elapsed: Rc::new(Cell::new(Duration::ZERO)),
    }))
    .unwrap();
    let ui = MainWindow::new().unwrap();
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (sender, mut commands) = unbounded_channel();
    let (updates, _receiver) = mpsc::channel();
    let mut host = Host {
        editor: Editor::new(Config::default()),
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
    host.editor
        .set_field("details", "Keep this unsaved profile draft")
        .unwrap();
    ui.set_global_id_draft("123456789012345678".into());
    host.refresh(&ui);
    host.action(&ui, "set-appearance", "light");
    let Command::Save {
        config, selected, ..
    } = commands.try_recv().unwrap()
    else {
        panic!("expected appearance save");
    };
    assert_eq!(config.appearance, Appearance::Light);
    assert!(selected.is_none());
    assert!(
        ui.get_dark_appearance(),
        "a pending or failed save keeps the saved theme"
    );
    assert_eq!(host.editor.config.appearance, Appearance::Dark);

    host.editor.accept(config, None);
    host.refresh(&ui);
    slint::platform::update_timers_and_animations();
    assert!(!ui.get_dark_appearance());
    assert_eq!(host.editor.draft.details, "Keep this unsaved profile draft");
    assert!(host.editor.dirty());
    assert_eq!(ui.get_global_id_draft(), "123456789012345678");
}

#[test]
fn activity_clock_updates_live_totals_and_moves_only_finished_sessions_into_history() {
    slint::platform::set_platform(Box::new(TestPlatform {
        window: MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer),
        elapsed: Rc::new(Cell::new(Duration::ZERO)),
    }))
    .unwrap();
    let ui = MainWindow::new().unwrap();
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (sender, _commands) = unbounded_channel();
    let (updates, _receiver) = mpsc::channel();
    let mut host = Host {
        editor: Editor::new(Config::default()),
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
    host.runtime.usage_totals.insert("editor".into(), 10_000);
    host.runtime.usage_sessions = vec![
        UsageSession {
            id: "active".into(),
            rule_id: "editor".into(),
            name: "Editor".into(),
            process: "Code.exe".into(),
            started_at: 1_000,
            ended_at: 3_000,
            duration_ms: 2_000,
            ..UsageSession::default()
        },
        UsageSession {
            id: "completed".into(),
            duration_ms: 8_000,
            ..UsageSession::default()
        },
    ];
    host.runtime.recording = Some(Recording {
        session_id: "active".into(),
        rule_id: "editor".into(),
        since: 3_000,
    });
    host.refresh_at(&ui, 8_000);
    let total = ui.get_totals().row_data(0).unwrap();
    assert_eq!(total.time, "0:00:15");
    assert_eq!(total.subtitle, "Recording now · This session 0:00:07");
    assert!(total.locked);
    assert_eq!(ui.get_sessions().row_count(), 1);
    let completed = ui.get_sessions().row_data(0).unwrap();
    assert_eq!(completed.id, "completed");

    // No backend snapshot arrives between these clock ticks.
    host.refresh_at(&ui, 9_000);
    assert_eq!(ui.get_totals().row_data(0).unwrap().time, "0:00:16");
    assert_eq!(ui.get_sessions().row_data(0).unwrap(), completed);

    // A persisted checkpoint advances the baseline without counting twice.
    host.runtime.usage_totals.insert("editor".into(), 16_000);
    host.runtime.usage_sessions[0].duration_ms = 8_000;
    host.runtime.usage_sessions[0].ended_at = 9_000;
    host.runtime.recording.as_mut().unwrap().since = 9_000;
    host.refresh_at(&ui, 10_000);
    let total = ui.get_totals().row_data(0).unwrap();
    assert_eq!(total.time, "0:00:17");
    assert_eq!(total.subtitle, "Recording now · This session 0:00:09");
    assert_eq!(ui.get_sessions().row_count(), 1);

    host.runtime.usage_totals.insert("editor".into(), 17_500);
    host.runtime.usage_sessions[0].duration_ms = 9_500;
    host.runtime.usage_sessions[0].ended_at = 10_500;
    host.runtime.recording = None;
    host.refresh_at(&ui, 10_500);
    assert_eq!(ui.get_sessions().row_count(), 2);
    let finished = ui.get_sessions().row_data(0).unwrap();
    assert_eq!(finished.id, "active");
    assert_eq!(finished.time, "0:00:09");
    assert!(!finished.locked);
    assert!(!ui.get_totals().row_data(0).unwrap().locked);
    host.refresh_at(&ui, 70_000);
    assert_eq!(ui.get_totals().row_data(0).unwrap().time, "0:00:17");
    assert_eq!(ui.get_sessions().row_data(0).unwrap(), finished);
    assert_eq!(ui.get_sessions().row_count(), 2);
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
