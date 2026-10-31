//! Opt-in smoke run against real native callbacks and the actual asynchronous host.
//! `--smoke-test` always creates its own workspace beneath `artifacts/native-smoke`.
use crate::{now_ms, AppTray, Host, MainWindow};
use presence_core::{model::*, storage, Backend};
use serde_json::json;
use slint::{ComponentHandle, Model};
use std::{
    cell::RefCell,
    fs,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

const MARKER: &str = "Presence Control isolated native smoke workspace\n";
const STORED_RULE: &str = "smoke-stored-rule";
const STORED_SESSION: &str = "smoke-stored-session";

pub fn prepare() -> Result<Option<PathBuf>, String> {
    if !std::env::args().any(|arg| arg == "--smoke-test") {
        return Ok(None);
    }
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let folder = repository
        .join("artifacts/native-smoke")
        .join(uuid::Uuid::new_v4().to_string());
    let data = folder.join("data");
    fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    fs::write(folder.join("SMOKE-WORKSPACE"), MARKER).map_err(|e| e.to_string())?;
    let mut config = default_config();
    config
        .extra
        .insert("smokeUnknownWorkspace".into(), json!({"preserved": true}));
    config.profiles[0]
        .extra
        .insert("smokeUnknownProfile".into(), json!(42));
    config.rules.push(AppRule {
        id: STORED_RULE.into(),
        name: "Stored editor".into(),
        process: "presence-smoke-never-running.exe".into(),
        profile_id: config.profiles[1].id.clone(),
        enabled: true,
        extra: serde_json::Map::from_iter([("smokeUnknownRule".into(), json!(true))]),
        ..AppRule::default()
    });
    let now = now_ms();
    let usage = UsageData {
        totals: std::collections::BTreeMap::from([(STORED_RULE.into(), 12_000)]),
        sessions: vec![UsageSession {
            id: STORED_SESSION.into(),
            rule_id: STORED_RULE.into(),
            name: "Stored editor".into(),
            process: "presence-smoke-never-running.exe".into(),
            started_at: now - 60_000,
            ended_at: now - 48_000,
            duration_ms: 12_000,
            extra: Default::default(),
        }],
        total_ids: None,
    };
    let archive = ArchiveData {
        entries: vec![ArchivedActivity {
            kind: "event".into(),
            archived_at: now - 1_000,
            entry: json!({"id":"smoke-archived-event","at":now - 2_000,"level":"success","message":"Saved activity survives a native restart."}),
            extra: Default::default(),
        }],
    };
    storage::atomic_json(&data.join("config.json"), &config)?;
    storage::atomic_json(&data.join("usage.json"), &usage)?;
    storage::atomic_json(&data.join("archive.json"), &archive)?;
    let data = data.canonicalize().map_err(|e| e.to_string())?;
    eprintln!("Native smoke workspace: {}", data.display());
    Ok(Some(data))
}

pub struct SmokeRun {
    timer: slint::Timer,
    state: Rc<RefCell<State>>,
}

impl SmokeRun {
    pub fn finish(self) -> Result<(), String> {
        self.timer.stop();
        let outcome = self.state.borrow().outcome.clone().unwrap_or_else(|| {
            Err("Native smoke window closed before all checks completed.".into())
        });
        outcome
    }
}

struct State {
    root: PathBuf,
    folder: PathBuf,
    step: u32,
    started: Instant,
    checks: Vec<String>,
    created_profile: String,
    new_rule: String,
    event_id: String,
    fault_active: bool,
    notification_started: Option<Instant>,
    empty_snapshot: Option<(Vec<AppRule>, slint::PhysicalSize)>,
    empty_snapshots_done: bool,
    outcome: Option<Result<(), String>>,
}

pub fn schedule(
    ui: &MainWindow,
    tray: &AppTray,
    host: Rc<RefCell<Host>>,
    root: PathBuf,
) -> SmokeRun {
    let folder = root.parent().unwrap_or(&root).to_path_buf();
    let state = Rc::new(RefCell::new(State {
        root,
        folder,
        step: 0,
        started: Instant::now(),
        checks: Vec::new(),
        created_profile: String::new(),
        new_rule: String::new(),
        event_id: String::new(),
        fault_active: false,
        notification_started: None,
        empty_snapshot: None,
        empty_snapshots_done: false,
        outcome: None,
    }));
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    let tray_weak = tray.as_weak();
    let runner = state.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(180),
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let Some(tray) = tray_weak.upgrade() else {
                return;
            };
            let mut state = runner.borrow_mut();
            if state.outcome.is_some() {
                return;
            }
            // High-DPI software snapshots plus the real notification wait can
            // exceed a minute on a busy machine.
            let result = if state.started.elapsed() > Duration::from_secs(90) {
                Err(format!("Native smoke timed out at step {}.", state.step))
            } else {
                state.tick(&ui, &tray, &host)
            };
            if let Err(error) = result {
                let error = format!("Native smoke step {} failed: {error}", state.step);
                eprintln!("{error}");
                let _ = state.snapshot(&ui, "failure");
                let _ = state.restore_fault();
                state.outcome = Some(Err(error));
                state.report();
                let _ = slint::quit_event_loop();
            }
        },
    );
    SmokeRun { timer, state }
}

impl State {
    fn check(&mut self, okay: bool, message: &str) -> Result<(), String> {
        if okay {
            self.checks.push(message.into());
            Ok(())
        } else {
            Err(message.into())
        }
    }

    fn snapshot(&self, ui: &MainWindow, name: &str) -> Result<(), String> {
        if !matches!(
            name,
            "validation" | "save-error" | "failure" | "unsaved" | "delete-profile" | "rule-dialog"
        ) {
            // Clear completed-operation notices so layout captures show the normal
            // product view. Error and confirmation snapshots retain their context.
            Self::action(ui, "dismiss-message", "");
        }
        let pixels = ui.window().take_snapshot().map_err(|e| e.to_string())?;
        image::save_buffer(
            self.folder.join(format!("slint-{name}.png")),
            pixels.as_bytes(),
            pixels.width(),
            pixels.height(),
            image::ColorType::Rgba8,
        )
        .map_err(|e| e.to_string())
    }

    fn config(&self) -> Result<Config, String> {
        storage::read_config_import(&self.root.join("config.json"))
    }

    /// Exercise live and completed projections in the native renderer with
    /// synthetic data only. The backend and its persisted usage stay untouched.
    fn activity_recording(
        &mut self,
        ui: &MainWindow,
        host: &Rc<RefCell<Host>>,
    ) -> Result<(), String> {
        let saved_runtime = host.borrow().runtime.clone();
        let result = (|| {
            let now = now_ms();
            {
                let mut host = host.borrow_mut();
                host.runtime
                    .usage_totals
                    .insert("smoke-live".into(), 3_661_000);
                host.runtime
                    .usage_totals
                    .insert("smoke-short".into(), 15_000);
                host.runtime.usage_sessions.insert(
                    0,
                    UsageSession {
                        id: "smoke-live-session".into(),
                        rule_id: "smoke-live".into(),
                        name: "Microsoft® Windows® Operating System".into(),
                        process: "ApplicationFrameHost.exe".into(),
                        started_at: now - 10_000,
                        ended_at: now - 2_000,
                        duration_ms: 8_000,
                        ..UsageSession::default()
                    },
                );
                host.runtime.usage_sessions.push(UsageSession {
                    id: "smoke-short-session".into(),
                    rule_id: "smoke-short".into(),
                    name: "App".into(),
                    process: "app.exe".into(),
                    started_at: now - 60_000,
                    ended_at: now - 45_000,
                    duration_ms: 15_000,
                    ..UsageSession::default()
                });
                host.runtime.recording = Some(Recording {
                    session_id: "smoke-live-session".into(),
                    rule_id: "smoke-live".into(),
                    since: now - 2_000,
                });
                host.refresh_at(ui, now);
            }
            let live = || {
                ui.get_totals()
                    .iter()
                    .find(|row| row.id == "smoke-live")
                    .unwrap()
            };
            self.check(
                live().time == "1:01:03" && live().locked,
                "Application time includes the current live interval",
            )?;
            self.check(
                ui.get_sessions()
                    .iter()
                    .all(|row| row.id != "smoke-live-session"),
                "Previous sessions excludes the active recording",
            )?;
            self.snapshot(ui, "activity-recording")?;
            host.borrow_mut().refresh_at(ui, now + 1_000);
            self.check(
                live().time == "1:01:04" && live().subtitle.ends_with("0:00:11"),
                "Live total and session clocks advance without a backend checkpoint",
            )?;
            {
                let mut host = host.borrow_mut();
                host.runtime
                    .usage_totals
                    .insert("smoke-live".into(), 3_664_000);
                host.runtime.usage_sessions[0].duration_ms = 11_000;
                host.runtime.usage_sessions[0].ended_at = now + 1_000;
                host.runtime.recording = None;
                host.refresh_at(ui, now + 1_000);
            }
            self.check(
                ui.get_sessions().iter().any(|row| {
                    row.id == "smoke-live-session" && row.time == "0:00:11" && !row.locked
                }),
                "Finished recording appears in Previous sessions with its final duration",
            )?;
            host.borrow_mut().refresh_at(ui, now + 60_000);
            self.check(
                live().time == "1:01:04" && !live().locked,
                "Finished application time stops advancing",
            )?;
            self.snapshot(ui, "activity-finished")
        })();
        host.borrow_mut().runtime = saved_runtime;
        host.borrow_mut().refresh(ui);
        result
    }

    fn guard_root(&self) -> Result<(), String> {
        let marker =
            fs::read_to_string(self.folder.join("SMOKE-WORKSPACE")).map_err(|e| e.to_string())?;
        if marker != MARKER || self.root.file_name().is_none_or(|name| name != "data") {
            return Err("Smoke filesystem checks require a marked isolated data directory.".into());
        }
        Ok(())
    }

    fn restore_fault(&mut self) -> Result<(), String> {
        if self.fault_active {
            self.guard_root()?;
            let path = self.root.join("config.json");
            // This directory was created empty by this exact smoke run. No recursive removal.
            if path.is_dir() {
                fs::remove_dir(&path).map_err(|e| e.to_string())?;
            }
            fs::rename(self.root.join("smoke-saved-config.json"), &path)
                .map_err(|e| e.to_string())?;
            self.fault_active = false;
        }
        Ok(())
    }

    fn report(&self) {
        let passed = self.outcome.as_ref().is_some_and(Result::is_ok);
        let error = self
            .outcome
            .as_ref()
            .and_then(|result| result.as_ref().err());
        let report = json!({
            "passed":passed, "error":error, "step":self.step,
            "elapsedSeconds":self.started.elapsed().as_secs_f32(), "checks":self.checks,
            "isolatedDataDirectory":self.root,
        });
        if let Ok(bytes) = serde_json::to_vec_pretty(&report) {
            let _ = fs::write(self.folder.join("smoke-report.json"), bytes);
        }
    }

    fn edit(ui: &MainWindow, host: &Rc<RefCell<Host>>, field: &str, value: &str) {
        ui.invoke_field_edited(field.into(), value.into());
        // A user edit updates the bound form first. Explicitly echo that form in
        // this callback-driven run so screenshots exercise the same visible state.
        host.borrow().refresh_form(ui);
    }

    fn action(ui: &MainWindow, action: &str, argument: &str) {
        ui.invoke_action(action.into(), argument.into());
    }

    fn start_rule_drag(ui: &MainWindow, from_y: f32, to_y: f32) {
        use slint::{
            platform::{PointerEventButton, WindowEvent},
            LogicalPosition,
        };
        let start = LogicalPosition::new(294.0, from_y);
        ui.window()
            .dispatch_event(WindowEvent::PointerMoved { position: start });
        ui.window().dispatch_event(WindowEvent::PointerPressed {
            position: start,
            button: PointerEventButton::Left,
        });
        ui.window().dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(304.0, from_y),
        });
        ui.window().dispatch_event(WindowEvent::PointerMoved {
            position: LogicalPosition::new(600.0, to_y),
        });
    }

    fn finish_rule_drag(ui: &MainWindow, to_y: f32) {
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased {
                position: slint::LogicalPosition::new(600.0, to_y),
                button: slint::platform::PointerEventButton::Left,
            });
    }

    fn rule_edit(ui: &MainWindow, field: &str, value: &str) {
        ui.invoke_rule_edited(field.into(), value.into());
    }

    fn tick(
        &mut self,
        ui: &MainWindow,
        tray: &AppTray,
        host: &Rc<RefCell<Host>>,
    ) -> Result<(), String> {
        if ui.get_busy() {
            return Ok(());
        }
        match self.step {
            0 => {
                self.guard_root()?;
                self.check(
                    host.borrow().editor.config.profiles.len() == 3,
                    "Loads all inherited profiles",
                )?;
                self.check(
                    ui.get_profiles().row_count() == 3,
                    "Profile model reaches the native UI",
                )?;
                self.check(
                    host.borrow().runtime.usage_sessions.len() == 1,
                    "Loads persisted time sessions",
                )?;
                self.check(
                    host.borrow().runtime.archive.len() == 1,
                    "Loads persisted archive entries",
                )?;
                self.snapshot(ui, "studio")?;
                Self::action(ui, "search", "Deep");
                self.check(
                    ui.get_profiles().row_count() == 1,
                    "Profile search filters the native model",
                )?;
                Self::action(ui, "search", "");
                Self::edit(ui, host, "details", "Smoke {{app}}");
                self.check(
                    ui.get_preview_details() == "Smoke Visual Studio Code",
                    "Rust resolves preview template tokens",
                )?;
                Self::action(ui, "select-profile", "deep-work");
            }
            1 => {
                self.check(
                    ui.get_modal() == "unsaved",
                    "Selection prompts before discarding unsaved edits",
                )?;
                self.snapshot(ui, "unsaved")?;
                Self::action(ui, "modal-cancel", "");
                self.check(
                    host.borrow().editor.selected_id() == "creative-flow",
                    "Cancel preserves the selected profile",
                )?;
                Self::action(ui, "select-profile", "deep-work");
                Self::action(ui, "discard-changes", "");
                self.check(
                    host.borrow().editor.selected_id() == "deep-work",
                    "Discard completes pending profile selection",
                )?;
                Self::action(ui, "select-profile", "creative-flow");
                Self::edit(ui, host, "name", "Native smoke presence");
                Self::action(ui, "select-profile", "deep-work");
                self.check(
                    ui.get_modal() == "unsaved",
                    "Save-and-switch starts from the unsaved-edits dialog",
                )?;
                Self::action(ui, "save-and-continue", "");
            }
            2 => {
                self.check(
                    self.config()?.profiles[0].name == "Native smoke presence",
                    "Profile save persists through the backend",
                )?;
                self.check(
                    !host.borrow().editor.dirty(),
                    "Successful saves commit the editor draft",
                )?;
                self.check(
                    host.borrow().editor.selected_id() == "deep-work",
                    "Save-and-switch waits for persistence before completing selection",
                )?;
                Self::action(ui, "new-profile", "");
            }
            3 => {
                self.check(
                    host.borrow().editor.config.profiles.len() == 4,
                    "Creates a presence through UI callbacks",
                )?;
                self.created_profile = host.borrow().editor.selected_id().into();
                for (field, value) in [
                    ("name", "Slint smoke profile"),
                    ("appName", "{{app}}"),
                    ("type", "2"),
                    ("elapsed", "true"),
                    ("progress", "true"),
                    ("duration", "3:00"),
                    ("details", "Native media details"),
                    ("state", "Native media state"),
                    ("largeImage", "smoke_asset"),
                    ("largeText", "Artwork hover text"),
                    ("smallImage", "smoke_small_asset"),
                    ("smallText", "Small artwork hover"),
                    ("theme", "blue"),
                ] {
                    Self::edit(ui, host, field, value);
                }
                Self::action(ui, "add-button", "");
                Self::edit(ui, host, "button1Label", "First button");
                Self::edit(ui, host, "button1Url", "https://example.com/one");
                Self::action(ui, "add-button", "");
                Self::edit(ui, host, "button2Label", "Second button");
                Self::edit(ui, host, "button2Url", "https://example.com/two");
                Self::action(ui, "save-profile", "");
            }
            4 => {
                let config = self.config()?;
                let profile = config
                    .profiles
                    .iter()
                    .find(|p| p.id == self.created_profile)
                    .ok_or("New profile disappeared.")?;
                self.check(
                    profile.buttons.len() == 2
                        && profile.buttons[0].label == "First button"
                        && profile.buttons[1].label == "Second button",
                    "Both native button editors use the correct one-based keys",
                )?;
                self.check(
                    profile.progress && profile.duration == "3:00" && profile.r#type == 2,
                    "Playback controls persist",
                )?;
                self.check(
                    profile.large_image == "smoke_asset"
                        && profile.small_text == "Small artwork hover",
                    "Artwork and hover fields persist",
                )?;
                self.check(
                    config.extra["smokeUnknownWorkspace"]["preserved"] == true
                        && config.profiles[0].extra["smokeUnknownProfile"] == 42,
                    "Unknown inherited JSON fields survive native saves",
                )?;
                self.snapshot(ui, "media")?;
                ui.set_editor_tab(1);
            }
            5 => {
                self.snapshot(ui, "artwork")?;
                // Render opaque, non-square artwork through the production
                // pixel/cache/model path without depending on a remote host.
                let (config, draft) = {
                    let mut host = host.borrow_mut();
                    let original = (host.editor.config.clone(), host.editor.draft.clone());
                    let url = "https://smoke.invalid/rounded-artwork.png";
                    let selected = host.editor.draft.id.clone();
                    for profile in &mut host.editor.config.profiles {
                        if profile.id == selected {
                            profile.large_image = url.into();
                        }
                    }
                    host.editor.draft.large_image = url.into();
                    host.editor.draft.small_image = url.into();
                    let source = image::RgbaImage::from_fn(192, 128, |x, y| {
                        image::Rgba([230, (80 + y).min(255) as u8, (x / 2) as u8, 255])
                    });
                    host.receive_artwork(
                        url.into(),
                        Ok(crate::artwork::Pixels {
                            width: 192,
                            height: 128,
                            rgba: source.into_raw(),
                        }
                        .rounded()),
                    );
                    host.refresh_form(ui);
                    host.refresh(ui);
                    original
                };
                self.check(
                    ui.get_has_preview_large() && ui.get_has_preview_small(),
                    "Native artwork reaches the editor, preview, and badge",
                )?;
                self.check(
                    (0..ui.get_profiles().row_count()).any(|i| {
                        let row = ui.get_profiles().row_data(i).unwrap();
                        row.selected && row.has_artwork && row.artwork.size().width == 126
                    }),
                    "Saved presence card uses the loaded artwork",
                )?;
                self.snapshot(ui, "rounded-artwork")?;
                {
                    let mut host = host.borrow_mut();
                    host.editor.config = config;
                    host.editor.draft = draft;
                    host.refresh_form(ui);
                    host.refresh(ui);
                }
                ui.set_editor_tab(2);
            }
            6 => {
                self.snapshot(ui, "buttons")?;
                ui.set_editor_tab(3);
            }
            7 => {
                self.snapshot(ui, "connection")?;
                Self::action(ui, "duplicate-profile", "");
            }
            8 => {
                self.check(
                    host.borrow().editor.config.profiles.len() == 5,
                    "Duplicates the full edited presence",
                )?;
                self.check(
                    host.borrow().editor.draft.buttons.len() == 2,
                    "Duplicate retains both buttons",
                )?;
                Self::action(ui, "delete-profile", "");
            }
            9 => {
                self.check(
                    ui.get_modal() == "delete-profile",
                    "Deleting a profile uses a confirmation dialog",
                )?;
                self.snapshot(ui, "delete-profile")?;
                Self::action(ui, "confirm-delete-profile", "");
            }
            10 => {
                ui.set_page(1);
                if !self.empty_snapshots_done {
                    if let Some((rules, size)) = self.empty_snapshot.take() {
                        // Wait a native event-loop turn after resizing before
                        // capturing the narrower layout, then wait again to restore.
                        self.snapshot(ui, "integrations-empty-narrow")?;
                        ui.window().set_size(size);
                        let mut host = host.borrow_mut();
                        host.editor.config.rules = rules;
                        host.refresh(ui);
                        self.empty_snapshots_done = true;
                    } else {
                        let rules = {
                            let mut host = host.borrow_mut();
                            let saved = std::mem::take(&mut host.editor.config.rules);
                            host.refresh(ui);
                            saved
                        };
                        self.snapshot(ui, "integrations-empty")?;
                        self.empty_snapshot = Some((rules, ui.window().size()));
                        ui.window().set_size(slint::LogicalSize::new(900.0, 800.0));
                    }
                    return Ok(());
                }
                self.check(
                    self.config()?.profiles.len() == 4,
                    "Confirmed deletion persists",
                )?;
                Self::action(ui, "new-rule", "");
                Self::rule_edit(ui, "name", "Native smoke rule");
                Self::rule_edit(ui, "process", "presence-smoke-never-running-2.exe");
                Self::rule_edit(ui, "titleContains", "Smoke");
                Self::rule_edit(ui, "profileIndex", "1");
                self.new_rule = host
                    .borrow()
                    .rule
                    .as_ref()
                    .ok_or("Rule dialog missing.")?
                    .id
                    .clone();
                Self::action(ui, "save-rule", "");
            }
            11 => {
                self.check(
                    self.config()?.rules.len() == 2,
                    "Native rule editor saves a new application",
                )?;
                self.check(
                    self.config()?.rules[1].profile_id == "deep-work",
                    "Rule profile selector resolves the selected profile",
                )?;
                Self::start_rule_drag(ui, 520.0, 416.0);
                self.snapshot(ui, "integrations-drag")?;
                Self::finish_rule_drag(ui, 416.0);
            }
            12 => {
                self.check(
                    self.config()?.rules[0].id == self.new_rule,
                    "Dropping before an application persists matching priority",
                )?;
                Self::start_rule_drag(ui, 438.0, 540.0);
                Self::finish_rule_drag(ui, 540.0);
            }
            13 => {
                self.check(
                    self.config()?.rules[1].id == self.new_rule,
                    "Dropping after an application persists matching priority",
                )?;
                Self::action(ui, "toggle-rule", &self.new_rule);
            }
            14 => {
                self.check(
                    !self.config()?.rules[1].enabled,
                    "Application rule switch persists",
                )?;
                Self::action(ui, "edit-rule", &self.new_rule);
                Self::rule_edit(ui, "name", "Edited native smoke rule");
            }
            15 => {
                self.snapshot(ui, "rule-dialog")?;
                Self::action(ui, "save-rule", "");
            }
            16 => {
                self.check(
                    self.config()?.rules[1].name == "Edited native smoke rule",
                    "Existing rules can be edited",
                )?;
                Self::action(ui, "scan", "");
            }
            17 => {
                self.check(!ui.get_scanning(), "Native application scan completes")?;
                self.check(
                    ui.get_apps().row_count() == host.borrow().runtime.apps.len(),
                    "Window detection updates the UI model",
                )?;
                self.snapshot(ui, "integrations")?;
                ui.set_page(0);
                Self::action(ui, "activate", "");
                self.check(
                    ui.get_message().contains("application ID"),
                    "Go live validates the public Discord application ID",
                )?;
                Self::edit(ui, host, "name", "");
                Self::action(ui, "save-profile", "");
                self.check(
                    ui.get_message().contains("name"),
                    "Invalid drafts show actionable validation errors",
                )?;
            }
            18 => {
                self.snapshot(ui, "validation")?;
                Self::action(ui, "discard-changes", "");
                self.guard_root()?;
                fs::rename(
                    self.root.join("config.json"),
                    self.root.join("smoke-saved-config.json"),
                )
                .map_err(|e| e.to_string())?;
                self.fault_active = true;
                fs::create_dir(self.root.join("config.json")).map_err(|e| e.to_string())?;
                Self::edit(ui, host, "details", "This write must fail");
                Self::action(ui, "save-profile", "");
            }
            19 => {
                self.check(
                    host.borrow().editor.dirty(),
                    "Failed persistence keeps the draft unsaved",
                )?;
                self.check(
                    host.borrow().editor.config.profiles[0].details != "This write must fail",
                    "Failed persistence does not mutate saved host state",
                )?;
                self.check(
                    ui.get_message().contains("Could not"),
                    "Atomic-save errors are surfaced in the UI",
                )?;
                self.snapshot(ui, "save-error")?;
                self.restore_fault()?;
                Self::action(ui, "discard-changes", "");
                Self::action(ui, "toggle-automatic", "");
            }
            20 => {
                self.check(
                    self.config()?.automatic,
                    "Automatic mode persists through its native switch",
                )?;
                Self::action(ui, "toggle-recording", "");
            }
            21 => {
                self.check(
                    self.config()?.record_time,
                    "Recording preference persists through its native switch",
                )?;
                ui.set_page(2);
            }
            22 => {
                self.snapshot(ui, "activity")?;
                self.activity_recording(ui, host)?;
                ui.set_page(3);
            }
            23 => {
                self.snapshot(ui, "archive")?;
                ui.set_page(4);
            }
            24 => {
                self.snapshot(ui, "settings")?;
                ui.set_page(0);
                ui.set_editor_tab(0);
                ui.window().set_size(slint::PhysicalSize::new(1040, 720));
            }
            25 => {
                self.snapshot(ui, "narrow")?;
                ui.window().set_size(slint::PhysicalSize::new(1440, 980));
                Self::action(ui, "stop", "");
            }
            26 => {
                self.check(
                    !self.config()?.automatic,
                    "Stop disables automatic mode and saves the preference",
                )?;
                self.check(
                    host.borrow().runtime.connection == "disconnected",
                    "Stop leaves Discord disconnected",
                )?;
                self.event_id = host
                    .borrow()
                    .runtime
                    .logs
                    .first()
                    .ok_or("Stop did not create an activity event.")?
                    .id
                    .clone();
                Self::action(ui, "archive-activity", &format!("event:{}", self.event_id));
            }
            27 => {
                self.check(
                    host.borrow()
                        .runtime
                        .archive
                        .iter()
                        .any(|e| e.entry["id"] == self.event_id),
                    "Activity events can be archived through native callbacks",
                )?;
                Self::action(ui, "archive-activity", &format!("session:{STORED_SESSION}"));
            }
            28 => {
                self.check(
                    host.borrow().runtime.usage_sessions.is_empty(),
                    "Archiving a completed session removes it from recent history",
                )?;
                self.check(
                    host.borrow().runtime.usage_totals.get(STORED_RULE) == Some(&12_000),
                    "Archiving a session retains cumulative application time",
                )?;
                Self::action(
                    ui,
                    "archive-activity",
                    &format!("application:{STORED_RULE}"),
                );
            }
            29 => {
                self.check(
                    host.borrow().runtime.usage_totals.get(STORED_RULE) == Some(&12_000),
                    "Archiving an application keeps its cumulative total in Application time",
                )?;
                self.check(
                    host.borrow()
                        .runtime
                        .archive
                        .iter()
                        .any(|e| e.tracks_application_time() && e.entry["durationMs"] == 12_000),
                    "Archived application totals subscribe to completed recording updates",
                )?;
                self.check(
                    ui.get_totals()
                        .iter()
                        .any(|row| row.id == STORED_RULE && row.tracked),
                    "Application time marks a linked archive and prevents duplicate archiving",
                )?;
                ui.set_page(2);
                self.snapshot(ui, "application-time-linked")?;
                ui.set_page(3);
                self.snapshot(ui, "archive-linked-total")?;
                Self::action(ui, "delete-archived", "event:smoke-archived-event");
            }
            30 => {
                self.check(
                    !host
                        .borrow()
                        .runtime
                        .archive
                        .iter()
                        .any(|e| e.entry["id"] == "smoke-archived-event"),
                    "Archived entries can be permanently deleted",
                )?;
                Self::action(ui, "stop", "");
            }
            31 => {
                let id = host
                    .borrow()
                    .runtime
                    .logs
                    .first()
                    .ok_or("No recent event to delete.")?
                    .id
                    .clone();
                self.event_id = id.clone();
                Self::action(ui, "delete-activity", &format!("event:{id}"));
            }
            32 => {
                self.check(
                    !host
                        .borrow()
                        .runtime
                        .logs
                        .iter()
                        .any(|e| e.id == self.event_id),
                    "Recent activity deletion uses the original event ID",
                )?;
                Self::action(ui, "delete-rule", &self.new_rule);
            }
            33 => {
                self.check(
                    self.config()?.rules.len() == 1,
                    "Application rules can be deleted",
                )?;
                Self::action(ui, "toggle-recording", "");
            }
            34 => {
                self.check(
                    !self.config()?.record_time,
                    "Disabling recording preserves archived history",
                )?;
                ui.set_page(4);
                Self::action(ui, "edit-global-id", "invalid");
                Self::action(ui, "save-global-id", "");
            }
            35 => {
                self.check(
                    ui.get_message().contains("17–20 digits")
                        && ui.get_global_id().is_empty()
                        && ui.get_global_id_draft() == "invalid"
                        && ui.get_global_id_dirty(),
                    "Invalid global ID saves retain the draft without changing the saved ID",
                )?;
                Self::action(ui, "edit-global-id", " 1562501049397411983 ");
                self.check(
                    ui.get_global_id().is_empty()
                        && host.borrow().editor.config.application_id.is_empty(),
                    "Typing a global ID does not display it as a saved connection setting",
                )?;
                Self::edit(ui, host, "details", "Profile draft survives Settings save");
                Self::action(ui, "save-global-id", "");
            }
            36 => {
                self.check(
                    self.config()?.application_id == "1562501049397411983"
                        && ui.get_global_id() == "1562501049397411983"
                        && ui.get_global_id_draft() == "1562501049397411983"
                        && !ui.get_global_id_dirty(),
                    "Saving the global ID persists and normalizes the Settings field and connection label",
                )?;
                self.check(
                    host.borrow().editor.dirty()
                        && host.borrow().editor.draft.details == "Profile draft survives Settings save"
                        && host.borrow().editor.draft.application_id.is_empty()
                        && host.borrow().editor.preview(&host.borrow().runtime, now_ms()).effective_application_id == "1562501049397411983",
                    "Settings save preserves profile edits and blank overrides inherit the saved global ID",
                )?;
                Self::action(ui, "discard-changes", "");
                Self::action(ui, "edit-global-id", "987654321098765432");
                Self::action(ui, "toggle-recording", "");
            }
            37 => {
                self.check(
                    ui.get_global_id() == "1562501049397411983"
                        && ui.get_global_id_draft() == "987654321098765432"
                        && ui.get_global_id_dirty(),
                    "Unrelated workspace saves preserve pending global ID edits",
                )?;
                Self::action(ui, "toggle-recording", "");
            }
            38 => {
                self.guard_root()?;
                fs::rename(
                    self.root.join("config.json"),
                    self.root.join("smoke-saved-config.json"),
                )
                .map_err(|e| e.to_string())?;
                self.fault_active = true;
                fs::create_dir(self.root.join("config.json")).map_err(|e| e.to_string())?;
                // Persistence must fail before activation can reach real Discord.
                Self::action(ui, "activate", "");
                self.check(
                    ui.get_busy(),
                    "Go live accepts the pending global ID for an inherited presence",
                )?;
            }
            39 => {
                self.check(
                    ui.get_message().contains("Could not")
                        && ui.get_global_id() == "1562501049397411983"
                        && ui.get_global_id_draft() == "987654321098765432"
                        && ui.get_global_id_dirty()
                        && host.borrow().runtime.connection == "disconnected",
                    "Failed Go live persistence retains pending global ID edits and does not connect to Discord",
                )?;
                self.restore_fault()?;
                Self::action(ui, "save-global-id", "");
            }
            40 => {
                self.check(
                    self.config()?.application_id == "987654321098765432"
                        && ui.get_global_id() == "987654321098765432"
                        && !ui.get_global_id_dirty()
                        && host.borrow().editor.candidate_activate(&ui.get_global_id_draft()).is_ok(),
                    "Retry saves the global ID and the inherited presence passes Go live validation",
                )?;
                let expected_config = host.borrow().editor.config.clone();
                let expected_archive = host.borrow().runtime.archive.clone();
                let expected_totals = host.borrow().runtime.usage_totals.clone();
                let executor = host.borrow().executor.clone();
                let root = self.root.clone();
                let (config, runtime, warnings) = executor.block_on(async move {
                    let (reopened, warnings) = Backend::open(&root, Arc::new(|_| {})).await;
                    let config = reopened.config().await;
                    let runtime = reopened.runtime().await;
                    reopened.shutdown().await?;
                    Ok::<_, String>((config, runtime, warnings))
                })?;
                self.check(
                    warnings.is_empty(),
                    "Native workspace reopens without recovery warnings",
                )?;
                self.check(
                    config == expected_config,
                    "Reopened backend restores the complete native configuration",
                )?;
                self.check(
                    runtime.archive == expected_archive,
                    "Reopened backend restores session, event, and total archives",
                )?;
                self.check(
                    runtime.usage_sessions.is_empty()
                        && runtime.usage_totals == expected_totals
                        && runtime.usage_totals.get(STORED_RULE) == Some(&12_000),
                    "Linked application totals and archived session removal survive restart",
                )?;
                Self::action(ui, "delete-activity", &format!("application:{STORED_RULE}"));
            }
            41 => {
                self.check(
                    !host.borrow().runtime.usage_totals.contains_key(STORED_RULE)
                        && ui.get_totals().row_count() == 0,
                    "Delete removes the Application time row",
                )?;
                self.check(
                    host.borrow().runtime.archive.iter().any(|item| {
                        item.kind == "application"
                            && !item.tracks_application_time()
                            && item.entry["durationMs"] == 12_000
                    }),
                    "Deleting Application time retains its final saved archive",
                )?;
                ui.set_page(2);
                self.snapshot(ui, "application-time-deleted")?;
            }
            42 => {
                let executor = host.borrow().executor.clone();
                let root = self.root.clone();
                let runtime = executor.block_on(async move {
                    let (reopened, _) = Backend::open(&root, Arc::new(|_| {})).await;
                    let runtime = reopened.runtime().await;
                    reopened.shutdown().await?;
                    Ok::<_, String>(runtime)
                })?;
                self.check(
                    runtime.usage_totals.is_empty()
                        && runtime.archive.iter().any(|item| {
                            item.kind == "application"
                                && !item.tracks_application_time()
                                && item.entry["durationMs"] == 12_000
                        }),
                    "Restart preserves deleted application totals and their detached archive",
                )?;
                crate::notify(ui, &host.borrow().notification_timer, "Presence stopped.");
                self.notification_started = Some(Instant::now());
            }
            43 => {
                if self.notification_started.unwrap().elapsed() < Duration::from_millis(5300) {
                    return Ok(());
                }
                self.check(
                    ui.get_message().is_empty(),
                    "Native status notification dismisses itself after five seconds",
                )?;
                self.check(
                    ui.get_dark_appearance(),
                    "Existing workspaces start in OLED dark mode",
                )?;
                ui.set_page(4);
                Self::action(ui, "set-appearance", "light");
            }
            44 => {
                self.check(
                    self.config()?.appearance == Appearance::Light && !ui.get_dark_appearance(),
                    "Light mode saves through the native host and updates the interface",
                )?;
                self.snapshot(ui, "light-settings")?;
                ui.set_page(0);
                ui.set_editor_tab(0);
            }
            45 => {
                self.snapshot(ui, "light-studio")?;
                ui.set_editor_tab(1);
            }
            46 => {
                self.snapshot(ui, "light-artwork")?;
                ui.set_page(1);
            }
            47 => {
                self.snapshot(ui, "light-integrations")?;
                Self::action(ui, "new-rule", "");
            }
            48 => {
                self.snapshot(ui, "light-rule-dialog")?;
                Self::action(ui, "modal-cancel", "");
                ui.set_page(2);
            }
            49 => {
                self.snapshot(ui, "light-activity")?;
                ui.set_page(3);
            }
            50 => {
                self.snapshot(ui, "light-archive")?;
                ui.set_page(4);
                ui.window().set_size(slint::PhysicalSize::new(800, 620));
            }
            51 => {
                self.snapshot(ui, "light-settings-narrow")?;
                Self::action(ui, "set-appearance", "system");
            }
            52 => {
                self.check(
                    self.config()?.appearance == Appearance::System
                        && ui.get_appearance() == "system",
                    "System mode persists the preference rather than the resolved OS color",
                )?;
                let (config, warnings) = host.borrow().executor.block_on(async {
                    let (backend, warnings) = Backend::open(&self.root, Arc::new(|_| {})).await;
                    let config = backend.config().await;
                    backend.shutdown().await?;
                    Ok::<_, String>((config, warnings))
                })?;
                self.check(
                    warnings.is_empty() && config.appearance == Appearance::System,
                    "A fresh backend reloads the saved appearance without data recovery",
                )?;
                Self::action(ui, "set-appearance", "dark");
            }
            53 => {
                self.check(
                    self.config()?.appearance == Appearance::Dark && ui.get_dark_appearance(),
                    "Dark mode restores the OLED black palette",
                )?;
                Self::edit(ui, host, "details", "Unsaved tray smoke draft");
                Self::action(ui, "set-appearance", "light");
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::CloseRequested);
                self.check(
                    !ui.window().is_visible()
                        && !host.borrow().shutting_down
                        && ui.get_modal().is_empty(),
                    "Closing during a save hides the studio without shutting down or prompting",
                )?;
            }
            54 => {
                self.check(
                    !ui.window().is_visible()
                        && self.config()?.appearance == Appearance::Light
                        && ui.get_appearance() == "light"
                        && host.borrow().editor.draft.details == "Unsaved tray smoke draft",
                    "The event loop and backend finish work while hidden and retain unsaved edits",
                )?;
                tray.invoke_open();
                self.check(
                    ui.window().is_visible()
                        && host.borrow().editor.draft.details == "Unsaved tray smoke draft",
                    "The tray Open command reopens the existing draft",
                )?;
            }
            55 => {
                ui.window().set_minimized(true);
            }
            56 => {
                tray.invoke_open();
                self.check(
                    ui.window().is_visible() && !ui.window().is_minimized(),
                    "The tray Open command restores a minimized window",
                )?;
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::CloseRequested);
                #[cfg(windows)]
                self.check(
                    crate::native::InstanceGuard::acquire(&self.root)?.is_none(),
                    "A second launch signals the hidden instance instead of creating another",
                )?;
                #[cfg(not(windows))]
                tray.invoke_open();
            }
            57 => {
                self.check(
                    ui.window().is_visible() && !host.borrow().shutting_down,
                    "Second-launch activation reopens the studio from the tray",
                )?;
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::CloseRequested);
                tray.invoke_quit();
                self.check(
                    ui.window().is_visible()
                        && ui.get_modal() == "unsaved"
                        && !host.borrow().shutting_down,
                    "Tray Quit reveals Save, Discard, and Cancel before exiting with an unsaved draft",
                )?;
                Self::action(ui, "modal-cancel", "");
                self.check(
                    ui.get_modal().is_empty()
                        && host.borrow().editor.dirty()
                        && !host.borrow().shutting_down,
                    "Canceling tray Quit keeps the application and unsaved edits alive",
                )?;
                Self::action(ui, "save-profile", "");
            }
            58 => {
                self.check(
                    !host.borrow().editor.dirty()
                        && self
                            .config()?
                            .profiles
                            .iter()
                            .any(|profile| profile.details == "Unsaved tray smoke draft"),
                    "A draft retained across close-to-tray can still be saved",
                )?;
                Self::action(ui, "set-appearance", "dark");
            }
            59 => {
                ui.window()
                    .dispatch_event(slint::platform::WindowEvent::CloseRequested);
                tray.invoke_quit();
                self.check(
                    !ui.window().is_visible() && host.borrow().shutting_down && ui.get_busy(),
                    "Tray Quit starts the checkpointing shutdown without reopening a clean studio",
                )?;
                self.outcome = Some(Ok(()));
                self.report();
                eprintln!(
                    "Native smoke passed {} checks. Artifacts: {}",
                    self.checks.len(),
                    self.folder.display()
                );
            }
            _ => return Err("Smoke state machine reached an unexpected step.".into()),
        }
        self.step += 1;
        Ok(())
    }
}
