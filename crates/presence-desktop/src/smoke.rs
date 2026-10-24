//! Opt-in smoke run against real native callbacks and the actual asynchronous host.
//! `--smoke-test` always creates its own workspace beneath `artifacts/native-smoke`.
use crate::{now_ms, Host, MainWindow};
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
    outcome: Option<Result<(), String>>,
}

pub fn schedule(ui: &MainWindow, host: Rc<RefCell<Host>>, root: PathBuf) -> SmokeRun {
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
        outcome: None,
    }));
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    let runner = state.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(180),
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut state = runner.borrow_mut();
            if state.outcome.is_some() {
                return;
            }
            let result = if state.started.elapsed() > Duration::from_secs(60) {
                Err(format!("Native smoke timed out at step {}.", state.step))
            } else {
                state.tick(&ui, &host)
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

    fn rule_edit(ui: &MainWindow, field: &str, value: &str) {
        ui.invoke_rule_edited(field.into(), value.into());
    }

    fn tick(&mut self, ui: &MainWindow, host: &Rc<RefCell<Host>>) -> Result<(), String> {
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
                self.check(
                    self.config()?.profiles.len() == 4,
                    "Confirmed deletion persists",
                )?;
                ui.set_page(1);
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
                Self::action(ui, "rule-up", &self.new_rule);
            }
            12 => {
                self.check(
                    self.config()?.rules[0].id == self.new_rule,
                    "Move up changes matching priority",
                )?;
                Self::action(ui, "rule-down", &self.new_rule);
            }
            13 => {
                self.check(
                    self.config()?.rules[1].id == self.new_rule,
                    "Move down changes matching priority",
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
                    !host.borrow().runtime.usage_totals.contains_key(STORED_RULE),
                    "Archiving a total resets the live cumulative total",
                )?;
                self.check(
                    host.borrow()
                        .runtime
                        .archive
                        .iter()
                        .any(|e| e.kind == "application" && e.entry["durationMs"] == 12_000),
                    "Archived totals retain their saved duration",
                )?;
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
                let expected_config = host.borrow().editor.config.clone();
                let expected_archive = host.borrow().runtime.archive.clone();
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
                    runtime.usage_sessions.is_empty() && runtime.usage_totals.is_empty(),
                    "Archived history stays removed from live totals and sessions after restart",
                )?;
                self.outcome = Some(Ok(()));
                self.report();
                eprintln!(
                    "Native smoke passed {} checks. Artifacts: {}",
                    self.checks.len(),
                    self.folder.display()
                );
                Self::action(ui, "quit", "");
            }
            _ => return Err("Smoke state machine reached an unexpected step.".into()),
        }
        self.step += 1;
        Ok(())
    }
}
