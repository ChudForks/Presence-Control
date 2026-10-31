#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod artwork;
mod editor;
mod native;
#[cfg(test)]
mod pointer_tests;
mod smoke;
#[cfg(test)]
mod ui_tests;
#[cfg(test)]
mod worker_tests;

use editor::Editor;
use presence_core::{
    engine,
    model::{AppRule, Appearance, Config, RuntimeState},
    Backend,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{mpsc, Arc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
slint::include_modules!();

enum Command {
    Save {
        config: Config,
        selected: Option<String>,
        activate: bool,
        notice: String,
    },
    Stop,
    Scan,
    Activity {
        operation: String,
        kind: String,
        id: String,
    },
    Import,
    Export,
    Shutdown,
}
enum Update {
    Runtime(Box<RuntimeState>),
    Saved {
        config: Box<Config>,
        selected: Option<String>,
    },
    Finished(Result<String, String>),
    Shutdown(Result<(), String>),
    Artwork(String, Result<artwork::ArtworkPixels, String>),
}
#[derive(Clone)]
enum Pending {
    Select(String),
    Close,
    Import,
}
struct Host {
    editor: Editor,
    runtime: RuntimeState,
    sender: UnboundedSender<Command>,
    updates: mpsc::Sender<Update>,
    executor: tokio::runtime::Handle,
    pending: Option<Pending>,
    rule: Option<AppRule>,
    resume_after_save: bool,
    shutting_down: bool,
    artwork: HashMap<String, artwork::ArtworkImages>,
    artwork_requests: HashMap<String, tokio::task::JoinHandle<()>>,
    artwork_retry_after: HashMap<String, std::time::Instant>,
    notification_timer: slint::Timer,
}

struct RuleDrag(SharedString);

fn rule_drag_data(id: SharedString) -> slint::DataTransfer {
    let mut data = slint::DataTransfer::default();
    data.set_user_data(Rc::new(RuleDrag(id)));
    data
}

fn rule_drag_id(data: slint::DataTransfer) -> SharedString {
    data.user_data()
        .and_then(|value| value.downcast_ref::<RuleDrag>().map(|drag| drag.0.clone()))
        .unwrap_or_default()
}

fn notify(ui: &MainWindow, timer: &slint::Timer, message: impl Into<SharedString>) {
    timer.stop();
    ui.set_message(message.into());
    if !ui.get_message().is_empty() {
        let weak = ui.as_weak();
        timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(5),
            move || {
                if let Some(ui) = weak.upgrade() {
                    ui.set_message("".into());
                }
            },
        );
    }
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(items)).into()
}
fn sync_model<T: Clone + PartialEq + 'static>(current: ModelRc<T>, items: Vec<T>) -> ModelRc<T> {
    if let Some(rows) = current.as_any().downcast_ref::<VecModel<T>>() {
        if rows.row_count() != items.len() {
            rows.set_vec(items);
        } else {
            for (index, item) in items.into_iter().enumerate() {
                if rows.row_data(index).as_ref() != Some(&item) {
                    rows.set_row_data(index, item);
                }
            }
        }
        current
    } else {
        model(items)
    }
}
fn profile_form(p: &presence_core::model::Profile) -> ProfileForm {
    ProfileForm {
        id: p.id.clone().into(),
        name: p.name.clone().into(),
        application_id: p.application_id.clone().into(),
        app_name: p.app_name.clone().into(),
        details: p.details.clone().into(),
        state: p.state.clone().into(),
        large_image: p.large_image.clone().into(),
        large_text: p.large_text.clone().into(),
        small_image: p.small_image.clone().into(),
        small_text: p.small_text.clone().into(),
        duration: p.duration.clone().into(),
        theme: p.theme.clone().into(),
        activity_type: i32::from(p.r#type),
        elapsed: p.elapsed,
        progress: p.progress,
        button_count: p.buttons.len() as i32,
        button1_label: p
            .buttons
            .first()
            .map(|b| b.label.clone())
            .unwrap_or_default()
            .into(),
        button1_url: p
            .buttons
            .first()
            .map(|b| b.url.clone())
            .unwrap_or_default()
            .into(),
        button2_label: p
            .buttons
            .get(1)
            .map(|b| b.label.clone())
            .unwrap_or_default()
            .into(),
        button2_url: p
            .buttons
            .get(1)
            .map(|b| b.url.clone())
            .unwrap_or_default()
            .into(),
    }
}
fn rule_form(rule: &AppRule, config: &Config) -> RuleForm {
    RuleForm {
        id: rule.id.clone().into(),
        name: rule.name.clone().into(),
        process: rule.process.clone().into(),
        title_contains: rule.title_contains.clone().into(),
        profile_id: rule.profile_id.clone().into(),
        profile_index: config
            .profiles
            .iter()
            .position(|p| p.id == rule.profile_id)
            .unwrap_or_default() as i32,
        enabled: rule.enabled,
    }
}
impl Host {
    fn reorder_rule(&self, ui: &MainWindow, id: &str, target_id: &str, after: bool) {
        if ui.get_busy() || !ui.get_modal().is_empty() {
            return;
        }
        match self.editor.candidate_rule_drop(id, target_id, after) {
            Ok(config) if config != self.editor.config => {
                self.save(ui, config, None, false, "Application priority updated.")
            }
            Ok(_) => {}
            Err(error) => self.error(ui, error),
        }
    }
    fn error(&self, ui: &MainWindow, error: impl Into<String>) {
        notify(ui, &self.notification_timer, error.into());
    }
    fn send(&self, ui: &MainWindow, command: Command) {
        if self.sender.send(command).is_ok() {
            ui.set_busy(true);
            notify(ui, &self.notification_timer, "");
        } else {
            self.error(
                ui,
                "The backend worker is unavailable. Close and reopen the application.",
            );
        }
    }
    fn save(
        &self,
        ui: &MainWindow,
        config: Config,
        selected: Option<String>,
        activate: bool,
        notice: &str,
    ) {
        self.send(
            ui,
            Command::Save {
                config,
                selected,
                activate,
                notice: notice.into(),
            },
        );
    }
    fn refresh_form(&self, ui: &MainWindow) {
        ui.set_form(profile_form(&self.editor.draft));
    }
    fn accept_saved(&mut self, ui: &MainWindow, config: Config, selected: Option<&str>) {
        let saved_rule = ui.get_modal() == "rule"
            && self
                .rule
                .as_ref()
                .is_some_and(|rule| config.rules.iter().any(|saved| saved == rule));
        let old_id = self.editor.config.application_id.clone();
        self.editor.accept(config, selected);
        if old_id != self.editor.config.application_id
            || ui.get_global_id_draft().trim() == self.editor.config.application_id
        {
            ui.set_global_id_draft(self.editor.config.application_id.clone().into());
        }
        if selected.is_some() {
            self.refresh_form(ui);
        }
        if saved_rule {
            self.rule = None;
            ui.set_modal("".into());
        }
    }
    fn finish(&mut self, ui: &MainWindow, result: Result<String, String>) {
        ui.set_busy(false);
        ui.set_scanning(false);
        match result {
            Ok(message) => {
                if !message.is_empty() {
                    notify(ui, &self.notification_timer, message);
                }
                if self.resume_after_save {
                    self.resume_after_save = false;
                    self.continue_pending(ui);
                }
            }
            Err(error) => {
                self.resume_after_save = false;
                self.error(ui, error);
            }
        }
    }
    fn refresh(&mut self, ui: &MainWindow) {
        self.refresh_at(ui, now_ms());
    }
    fn refresh_at(&mut self, ui: &MainWindow, now: i64) {
        ui.set_global_id(self.editor.config.application_id.clone().into());
        ui.set_appearance(self.editor.config.appearance.as_str().into());
        self.refresh_artwork(ui);
        let query = ui.get_search().to_lowercase();
        ui.set_profiles(sync_model(
            ui.get_profiles(),
            self.editor
                .config
                .profiles
                .iter()
                .filter(|p| p.name.to_lowercase().contains(&query))
                .map(|p| ProfileRow {
                    id: p.id.clone().into(),
                    title: p.name.clone().into(),
                    subtitle: format!("{} · Custom presence", editor::activity_label(p.r#type))
                        .into(),
                    theme: p.theme.clone().into(),
                    selected: p.id == self.editor.selected_id(),
                    artwork: self
                        .artwork
                        .get(&p.large_image)
                        .map(|images| images.card.clone())
                        .unwrap_or_default(),
                    has_artwork: self.artwork.contains_key(&p.large_image),
                })
                .collect(),
        ));
        ui.set_rules(sync_model(
            ui.get_rules(),
            self.editor
                .config
                .rules
                .iter()
                .map(|r| RuleRow {
                    id: r.id.clone().into(),
                    name: r.name.clone().into(),
                    process: if r.title_contains.is_empty() {
                        r.process.clone()
                    } else {
                        format!("{} · {}", r.process, r.title_contains)
                    }
                    .into(),
                    profile: self
                        .editor
                        .config
                        .profiles
                        .iter()
                        .find(|p| p.id == r.profile_id)
                        .map(|p| p.name.clone())
                        .unwrap_or_default()
                        .into(),
                    enabled: r.enabled,
                })
                .collect(),
        ));
        ui.set_profile_names(sync_model(
            ui.get_profile_names(),
            self.editor
                .config
                .profiles
                .iter()
                .map(|p| SharedString::from(&p.name))
                .collect(),
        ));
        ui.set_apps(sync_model(
            ui.get_apps(),
            self.runtime
                .apps
                .iter()
                .map(|app| AppRow {
                    id: app.pid.to_string().into(),
                    process: app.process.clone().into(),
                    title: app.title.clone().into(),
                })
                .collect(),
        ));
        ui.set_dirty(self.editor.dirty());
        ui.set_selected_live(
            self.runtime.active_profile_id.as_deref() == Some(self.editor.selected_id())
                && self.runtime.connection == "connected",
        );
        ui.set_automatic(self.editor.config.automatic);
        ui.set_record_time(self.editor.config.record_time);
        ui.set_connection(self.runtime.connection.clone().into());
        let active = self
            .runtime
            .active_profile_id
            .as_ref()
            .and_then(|id| self.editor.config.profiles.iter().find(|p| &p.id == id));
        ui.set_status(
            self.runtime
                .error
                .clone()
                .unwrap_or_else(|| {
                    active
                        .map(|p| format!("Live · {}", p.name))
                        .unwrap_or_else(|| {
                            if engine::valid_application_id(engine::application_id_for(
                                &self.editor.draft,
                                &self.editor.config.application_id,
                            )) {
                                "Ready when you are".into()
                            } else {
                                "Application ID needed".into()
                            }
                        })
                })
                .into(),
        );
        let preview = self.editor.preview(&self.runtime, now);
        ui.set_preview_kind(
            if self.editor.draft.r#type == 2 {
                format!("Listening to {}", preview.app_name)
            } else {
                preview.activity_label.to_uppercase()
            }
            .into(),
        );
        ui.set_preview_name(preview.app_name.into());
        ui.set_preview_details(preview.details.into());
        ui.set_preview_state(preview.state.into());
        ui.set_preview_large_text(preview.large_text.into());
        ui.set_preview_fraction(preview.playback_fraction);
        ui.set_preview_duration(preview.playback_duration.into());
        ui.set_has_playback(preview.has_playback);
        ui.set_preview_time(
            if preview.has_playback {
                preview.playback_position
            } else {
                preview.elapsed_text
            }
            .into(),
        );
        let activity_row = |row: editor::ActivityRow| ActivityRow {
            id: row.id.into(),
            kind: row.kind.into(),
            title: row.title.into(),
            subtitle: row.subtitle.into(),
            time: row.time.into(),
            detail: row.detail.into(),
            locked: row.locked,
            tracked: row.tracked,
        };
        let (mut events, mut totals, mut sessions) = (Vec::new(), Vec::new(), Vec::new());
        // Project current history once: resolving total names also scans the
        // permanent archive, so repeating it for each section grows expensive.
        for row in editor::activity_rows(&self.editor.config, &self.runtime, "all", "", false, now)
        {
            let target = match row.kind.as_str() {
                "event" => &mut events,
                "application" => &mut totals,
                "session" => &mut sessions,
                _ => continue,
            };
            target.push(activity_row(row));
        }
        ui.set_events(sync_model(ui.get_events(), events));
        ui.set_totals(sync_model(ui.get_totals(), totals));
        ui.set_sessions(sync_model(ui.get_sessions(), sessions));
        let archived =
            editor::activity_rows(&self.editor.config, &self.runtime, "all", "", true, now)
                .into_iter()
                .map(activity_row)
                .collect();
        ui.set_archived(sync_model(ui.get_archived(), archived));
    }
    fn refresh_artwork(&mut self, ui: &MainWindow) {
        let query = ui.get_search().to_lowercase();
        let mut wanted = vec![
            self.editor.draft.large_image.clone(),
            self.editor.draft.small_image.clone(),
        ];
        for profile in &self.editor.config.profiles {
            if profile.name.to_lowercase().contains(&query)
                && !wanted.contains(&profile.large_image)
            {
                wanted.push(profile.large_image.clone());
            }
        }
        // A workspace has at most 100 profiles. Keep only images used by its
        // visible cards or the draft, and limit concurrent network/decoder work.
        self.artwork.retain(|url, _| wanted.contains(url));
        let now = std::time::Instant::now();
        self.artwork_retry_after
            .retain(|url, deadline| *deadline > now && wanted.contains(url));
        self.artwork_requests.retain(|url, task| {
            if !wanted.contains(url) {
                task.abort();
                return false;
            }
            // Keep finished requests until their queued result is received, so
            // a refresh between completion and delivery cannot start a duplicate.
            true
        });
        let large = self.artwork.get(&self.editor.draft.large_image);
        ui.set_preview_large(
            large
                .map(|images| images.preview.clone())
                .unwrap_or_default(),
        );
        ui.set_editor_artwork(
            large
                .map(|images| images.editor.clone())
                .unwrap_or_default(),
        );
        ui.set_has_preview_large(large.is_some());
        let small = self.artwork.get(&self.editor.draft.small_image);
        ui.set_preview_small(small.map(|images| images.badge.clone()).unwrap_or_default());
        ui.set_has_preview_small(small.is_some());

        for url in wanted {
            if self.artwork_requests.len() >= 4 {
                break;
            }
            if !url.starts_with("https://")
                || self.artwork.contains_key(&url)
                || self.artwork_retry_after.contains_key(&url)
                || self.artwork_requests.contains_key(&url)
            {
                continue;
            }
            let tx = self.updates.clone();
            let requested_url = url.clone();
            let task = self.executor.spawn(async move {
                // Wait for typing to settle. Unused URLs are aborted above;
                // cards and previews sharing a URL use one request and cache.
                tokio::time::sleep(Duration::from_millis(350)).await;
                let result = artwork::fetch(&requested_url).await;
                let _ = tx.send(Update::Artwork(requested_url, result));
            });
            self.artwork_requests.insert(url, task);
        }
    }

    fn receive_artwork(&mut self, url: String, result: Result<artwork::ArtworkPixels, String>) {
        self.artwork_requests.remove(&url);
        match result {
            Ok(pixels) => {
                self.artwork_retry_after.remove(&url);
                self.artwork.insert(url, pixels.into());
            }
            Err(_) => {
                // A failed URL can be retried later instead of remaining in a
                // permanent in-flight set. Bound this map as well as the cache.
                if self.artwork_retry_after.len() >= 64 {
                    if let Some(old) = self.artwork_retry_after.keys().next().cloned() {
                        self.artwork_retry_after.remove(&old);
                    }
                }
                self.artwork_retry_after
                    .insert(url, std::time::Instant::now() + Duration::from_secs(30));
            }
        }
    }
    fn continue_pending(&mut self, ui: &MainWindow) {
        match self.pending.take() {
            Some(Pending::Select(id)) => {
                self.editor.discard();
                if let Err(error) = self.editor.select(&id) {
                    self.error(ui, error);
                }
                self.refresh_form(ui);
                ui.set_editor_tab(0);
            }
            Some(Pending::Close) => {
                self.shutting_down = true;
                self.send(ui, Command::Shutdown);
            }
            Some(Pending::Import) => self.send(ui, Command::Import),
            None => {}
        }
        ui.set_modal("".into());
    }
    fn request_pending(&mut self, ui: &MainWindow, pending: Pending) {
        self.pending = Some(pending);
        if self.editor.dirty() {
            ui.set_modal("unsaved".into());
        } else {
            self.continue_pending(ui);
        }
    }
    fn request_quit(&mut self, ui: &MainWindow) -> Result<(), slint::PlatformError> {
        if self.shutting_down || ui.get_busy() {
            return Ok(());
        }
        // A tray Quit must expose the existing Save/Discard/Cancel dialog when
        // a draft is present, without flashing the window for a clean shutdown.
        if self.editor.dirty() {
            native::show_main_window(ui.window())?;
        }
        self.request_pending(ui, Pending::Close);
        Ok(())
    }
    fn action(&mut self, ui: &MainWindow, action: &str, argument: &str) {
        if ui.get_busy() && !matches!(action, "dismiss-message" | "modal-cancel") {
            return;
        }
        let result: Result<(), String> = (|| {
            match action {
                "search" => ui.set_search(argument.into()),
                "edit-global-id" => ui.set_global_id_draft(argument.into()),
                "dismiss-message" => notify(ui, &self.notification_timer, ""),
                "select-profile" => {
                    if argument != self.editor.draft.id {
                        self.request_pending(ui, Pending::Select(argument.into()));
                    }
                }
                "new-profile" | "duplicate-profile" => {
                    let (config, id) = if action == "new-profile" {
                        self.editor.candidate_new()?
                    } else {
                        self.editor.candidate_duplicate()?
                    };
                    self.save(
                        ui,
                        config,
                        Some(id),
                        false,
                        if action == "new-profile" {
                            "New presence created. Make it yours."
                        } else {
                            "Presence duplicated."
                        },
                    );
                    ui.set_page(0);
                    ui.set_editor_tab(0);
                }
                "delete-profile" => ui.set_modal("delete-profile".into()),
                "confirm-delete-profile" => {
                    let config = self.editor.candidate_delete()?;
                    let id = config.profiles.first().map(|p| p.id.clone());
                    self.save(ui, config, id, false, "Presence deleted.");
                    ui.set_modal("".into());
                }
                "save-profile" | "activate" | "save-and-continue" => {
                    let config = if action == "activate" {
                        self.editor.candidate_activate(&ui.get_global_id_draft())?
                    } else {
                        self.editor.candidate_save()?
                    };
                    if action == "save-and-continue" {
                        self.resume_after_save = true;
                    }
                    self.save(
                        ui,
                        config,
                        Some(self.editor.draft.id.clone()),
                        action == "activate",
                        if action == "activate" {
                            "Presence is live."
                        } else {
                            "All changes saved."
                        },
                    );
                }
                "stop" => self.send(ui, Command::Stop),
                "add-button" => {
                    self.editor.add_button()?;
                    self.refresh_form(ui);
                }
                "remove-button" => {
                    self.editor
                        .remove_button(argument.parse().map_err(|_| "Invalid button index.")?)?;
                    self.refresh_form(ui);
                }
                "toggle-automatic" | "toggle-recording" => {
                    let mut config = self.editor.config.clone();
                    if action == "toggle-automatic" {
                        config.automatic = !config.automatic;
                    } else {
                        config.record_time = !config.record_time;
                    }
                    self.save(ui, config, None, false, "Workspace updated.");
                }
                "save-global-id" => {
                    let mut config = self.editor.config.clone();
                    config.application_id = ui.get_global_id_draft().trim().into();
                    self.save(
                        ui,
                        config,
                        None,
                        false,
                        "Discord connection settings saved.",
                    );
                }
                "set-appearance" => {
                    let appearance = match argument {
                        "dark" => Appearance::Dark,
                        "light" => Appearance::Light,
                        "system" => Appearance::System,
                        _ => return Err("Choose Dark, Light, or System for the appearance.".into()),
                    };
                    if appearance != self.editor.config.appearance {
                        let mut config = self.editor.config.clone();
                        config.appearance = appearance;
                        self.save(ui, config, None, false, "Appearance saved.");
                    }
                }
                "new-rule" | "edit-rule" | "add-detected" => {
                    let mut rule = if action == "edit-rule" {
                        self.editor
                            .config
                            .rules
                            .iter()
                            .find(|r| r.id == argument)
                            .cloned()
                            .ok_or("This rule no longer exists.")?
                    } else {
                        self.editor.new_rule()
                    };
                    if action == "add-detected" {
                        let app = self
                            .runtime
                            .apps
                            .iter()
                            .find(|app| app.pid.to_string() == argument)
                            .ok_or("This application is no longer available. Scan again.")?;
                        rule.process = app.process.clone();
                        rule.name = app.name.clone().unwrap_or_else(|| app.process.clone());
                    }
                    ui.set_rule_form(rule_form(&rule, &self.editor.config));
                    self.rule = Some(rule);
                    ui.set_modal("rule".into());
                }
                "save-rule" => {
                    let rule = self.rule.clone().ok_or("No rule is being edited.")?;
                    let config = self.editor.candidate_rule_upsert(rule)?;
                    self.save(ui, config, None, false, "Application rule saved.");
                    // Keep the dialog and draft until persistence succeeds so
                    // a failed write can be retried without re-entering the rule.
                }
                "delete-rule" | "toggle-rule" | "rule-up" | "rule-down" => {
                    let config = match action {
                        "delete-rule" => self.editor.candidate_rule_delete(argument)?,
                        "toggle-rule" => self.editor.candidate_rule_toggle(argument)?,
                        "rule-up" => self.editor.candidate_rule_move(argument, -1)?,
                        _ => self.editor.candidate_rule_move(argument, 1)?,
                    };
                    self.save(ui, config, None, false, "Application rules updated.");
                }
                "scan" => {
                    ui.set_scanning(true);
                    self.send(ui, Command::Scan);
                }
                "archive-activity" | "delete-activity" | "delete-archived" => {
                    let (kind, id) = argument
                        .split_once(':')
                        .ok_or("Invalid activity reference.")?;
                    self.send(
                        ui,
                        Command::Activity {
                            operation: action.into(),
                            kind: kind.into(),
                            id: id.into(),
                        },
                    );
                }
                "export" => self.send(ui, Command::Export),
                "import" => self.request_pending(ui, Pending::Import),
                "developer-portal" => native::open_developer_portal()?,
                "discard-changes" => {
                    self.editor.discard();
                    self.refresh_form(ui);
                    self.continue_pending(ui);
                }
                "modal-cancel" => {
                    self.pending = None;
                    self.rule = None;
                    self.resume_after_save = false;
                    ui.set_modal("".into());
                }
                "quit" => self.request_quit(ui).map_err(|error| error.to_string())?,
                _ => return Err(format!("Unknown interface action: {action}")),
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.error(ui, error);
        }
        self.refresh(ui);
    }
}
async fn worker(
    backend: Backend,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<Command>,
    updates: mpsc::Sender<Update>,
) {
    while let Some(command) = commands.recv().await {
        let result = match command {
            Command::Save {
                config,
                selected,
                activate,
                notice,
            } => match backend
                .save_config(&serde_json::to_value(&config).expect("configuration is serializable"))
                .await
            {
                Ok(saved) => {
                    let _ = updates.send(Update::Saved {
                        config: Box::new(saved),
                        selected: selected.clone(),
                    });
                    if activate {
                        backend
                            .activate(selected.as_deref().unwrap_or_default())
                            .await
                            .map(|_| notice)
                    } else {
                        Ok(notice)
                    }
                }
                Err(error) => Err(error),
            },
            Command::Stop => {
                let result = backend.stop().await;
                // Stopping disables automatic mode even if persistence or the
                // Discord clear fails. Reflect the current in-memory setting.
                let _ = updates.send(Update::Saved {
                    config: Box::new(backend.config().await),
                    selected: None,
                });
                result.map(|()| "Presence stopped. Automatic mode is off.".into())
            }
            Command::Scan => backend
                .scan()
                .await
                .map(|apps| format!("Found {} windowed applications.", apps.len())),
            Command::Activity {
                operation,
                kind,
                id,
            } => {
                let result = match operation.as_str() {
                    "archive-activity" => backend.archive_activity(&kind, &id).await,
                    "delete-archived" => backend.delete_archived_activity(&kind, &id).await,
                    _ => backend.delete_activity(&kind, &id).await,
                };
                result.map(|state| {
                    let _ = updates.send(Update::Runtime(Box::new(state)));
                    if operation == "archive-activity" && kind == "application" {
                        "Application time added to Archive. It stays here and updates after each session.".into()
                    } else {
                        "Activity updated.".into()
                    }
                })
            }
            Command::Import => {
                let path = tokio::task::spawn_blocking(|| {
                    rfd::FileDialog::new()
                        .set_title("Import Presence Control workspace")
                        .add_filter("Presence configuration", &["json"])
                        .pick_file()
                })
                .await;
                match path {
                    Ok(Some(path)) => match backend.import_from(&path).await {
                        Ok(config) => {
                            let selected = config.profiles.first().map(|p| p.id.clone());
                            let _ = updates.send(Update::Saved {
                                config: Box::new(config),
                                selected,
                            });
                            Ok("Workspace imported. Automatic mode is off.".into())
                        }
                        Err(error) => Err(error),
                    },
                    Ok(None) => Ok(String::new()),
                    Err(error) => Err(error.to_string()),
                }
            }
            Command::Export => {
                let path = tokio::task::spawn_blocking(|| {
                    rfd::FileDialog::new()
                        .set_title("Export Presence Control workspace")
                        .set_file_name("presence-control.json")
                        .add_filter("Presence configuration", &["json"])
                        .save_file()
                })
                .await;
                match path {
                    Ok(Some(path)) => backend
                        .export_to(&path)
                        .await
                        .map(|_| "Workspace exported.".into()),
                    Ok(None) => Ok(String::new()),
                    Err(error) => Err(error.to_string()),
                }
            }
            Command::Shutdown => {
                let result = backend.shutdown().await;
                let success = result.is_ok();
                let _ = updates.send(Update::Shutdown(result));
                if success {
                    break;
                }
                // A failed usage checkpoint keeps the worker alive so another
                // close request can retry instead of losing the command channel.
                continue;
            }
        };
        let _ = updates.send(Update::Runtime(Box::new(backend.runtime().await)));
        let _ = updates.send(Update::Finished(result));
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let smoke_root = smoke::prepare()?;
    let root = match &smoke_root {
        Some(root) => root.clone(),
        None => native::data_directory()?,
    };
    std::fs::create_dir_all(&root)?;
    let root = root.canonicalize()?;
    let Some(instance) = native::InstanceGuard::acquire(&root)? else {
        return Ok(());
    };
    let instance = Rc::new(instance);
    let executor = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let (updates, receiver) = mpsc::channel();
    let on_change = updates.clone();
    let (backend, warnings) = executor.block_on(Backend::open(
        &root,
        Arc::new(move |runtime| {
            let _ = on_change.send(Update::Runtime(Box::new(runtime)));
        }),
    ));
    let config = executor.block_on(backend.config());
    let runtime = executor.block_on(backend.runtime());
    let (sender, commands) = unbounded_channel();
    executor.spawn(worker(backend.clone(), commands, updates.clone()));
    let ui = MainWindow::new()?;
    let tray = AppTray::new()?;
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    ui.on_rule_drag_data(rule_drag_data);
    ui.on_rule_drag_id(rule_drag_id);
    if smoke_root.is_none() {
        native::place_initial_window(ui.window());
    }
    ui.set_global_id_draft(config.application_id.clone().into());
    ui.set_data_path(root.display().to_string().into());
    let host = Rc::new(RefCell::new(Host {
        editor: Editor::new(config),
        runtime,
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
    }));
    notify(&ui, &host.borrow().notification_timer, warnings.join("\n"));
    host.borrow().refresh_form(&ui);
    host.borrow_mut().refresh(&ui);
    let weak = ui.as_weak();
    let handler = host.clone();
    ui.on_action(move |action, argument| {
        if let Some(ui) = weak.upgrade() {
            handler.borrow_mut().action(&ui, &action, &argument);
        }
    });
    let weak = ui.as_weak();
    let handler = host.clone();
    ui.on_rule_reordered(move |id, target_id, after| {
        if let Some(ui) = weak.upgrade() {
            handler.borrow().reorder_rule(&ui, &id, &target_id, after);
        }
    });
    let weak = ui.as_weak();
    let handler = host.clone();
    ui.on_field_edited(move |field, value| {
        if let Some(ui) = weak.upgrade() {
            let mut host = handler.borrow_mut();
            if !ui.get_busy() {
                if let Err(error) = host.editor.set_field(&field, &value) {
                    host.error(&ui, error);
                }
                host.refresh_form(&ui);
                host.refresh(&ui);
            }
        }
    });
    let weak = ui.as_weak();
    let handler = host.clone();
    ui.on_rule_edited(move |field, value| {
        if let Some(ui) = weak.upgrade() {
            let mut host = handler.borrow_mut();
            if ui.get_busy() {
                return;
            }
            let profile_id = if field == "profileIndex" {
                value
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| host.editor.config.profiles.get(i))
                    .map(|p| p.id.clone())
            } else {
                None
            };
            if let Some(rule) = host.rule.as_mut() {
                let result = if let Some(id) = profile_id {
                    rule.profile_id = id;
                    Ok(())
                } else {
                    editor::set_rule_field(rule, &field, &value)
                };
                if let Err(error) = result {
                    host.error(&ui, error);
                }
            }
            if let Some(rule) = &host.rule {
                ui.set_rule_form(rule_form(rule, &host.editor.config));
            }
        }
    });
    // Slint's visible native tray keeps the event loop alive after the last
    // window is hidden. Closing never stops the backend or discards a draft.
    ui.window()
        .on_close_requested(|| slint::CloseRequestResponse::HideWindow);
    let weak = ui.as_weak();
    let handler = host.clone();
    tray.on_open(move || {
        if let Some(ui) = weak.upgrade() {
            if let Err(error) = native::show_main_window(ui.window()) {
                handler.borrow().error(&ui, error.to_string());
            }
        }
    });
    let weak = ui.as_weak();
    let handler = host.clone();
    tray.on_quit(move || {
        if let Some(ui) = weak.upgrade() {
            handler.borrow_mut().action(&ui, "quit", "");
        }
    });
    let weak = ui.as_weak();
    let tray_weak = tray.as_weak();
    let handler = host.clone();
    let timer = slint::Timer::default();
    let mut last_second = 0;
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut host = handler.borrow_mut();
        if instance.poll_activation() {
            if let Err(error) = native::show_main_window(ui.window()) {
                host.error(&ui, error.to_string());
            }
        }
        let mut changed = false;
        while let Ok(update) = receiver.try_recv() {
            match update {
                Update::Runtime(runtime) => {
                    host.runtime = *runtime;
                    changed = true;
                }
                Update::Saved { config, selected } => {
                    host.accept_saved(&ui, *config, selected.as_deref());
                    changed = true;
                }
                Update::Finished(result) => {
                    host.finish(&ui, result);
                    changed = true;
                }
                Update::Shutdown(result) => {
                    if let Err(error) = result {
                        host.shutting_down = false;
                        ui.set_busy(false);
                        let _ = native::show_main_window(ui.window());
                        host.error(&ui, format!("Could not save application time: {error}. Choose Quit from the tray again to retry."));
                    } else {
                        let _ = slint::quit_event_loop();
                    }
                }
                Update::Artwork(url, result) => {
                    host.receive_artwork(url, result);
                    changed = true;
                }
            }
        }
        let second = now_ms() / 1000;
        if changed || second != last_second {
            last_second = second;
            host.refresh(&ui);
        }
        if let Some(tray) = tray_weak.upgrade() {
            tray.set_quit_enabled(!ui.get_busy() && !host.shutting_down);
        }
    });
    let smoke_run = smoke_root.map(|root| smoke::schedule(&ui, &tray, host.clone(), root));
    ui.run()?;
    executor.block_on(backend.shutdown())?;
    if let Some(smoke_run) = smoke_run {
        smoke_run.finish()?;
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("Presence Control: {error}");
        if !std::env::args().any(|arg| arg == "--smoke-test") {
            let _ = rfd::MessageDialog::new()
                .set_title("Presence Control")
                .set_description(error.to_string())
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
        std::process::exit(1);
    }
}
