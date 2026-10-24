#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod artwork;
mod editor;
mod native;
#[cfg(test)]
mod pointer_tests;
mod smoke;
#[cfg(test)]
mod worker_tests;

use editor::Editor;
use presence_core::{
    engine,
    model::{AppRule, Config, RuntimeState},
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
    Artwork(bool, String, Result<artwork::Pixels, String>),
}
#[derive(Clone)]
enum Pending {
    Select(String),
    Close,
    Import,
}
struct ArtworkRequest {
    url: String,
    task: tokio::task::JoinHandle<()>,
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
    artwork: HashMap<String, slint::Image>,
    artwork_requests: HashMap<bool, ArtworkRequest>,
    artwork_retry_after: HashMap<String, std::time::Instant>,
    large_uri: String,
    small_uri: String,
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
    fn error(&self, ui: &MainWindow, error: impl Into<String>) {
        ui.set_message(error.into().into());
    }
    fn send(&self, ui: &MainWindow, command: Command) {
        if self.sender.send(command).is_ok() {
            ui.set_busy(true);
            ui.set_message("".into());
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
    fn refresh(&mut self, ui: &MainWindow) {
        ui.set_global_id(self.editor.config.application_id.clone().into());
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
        let preview = self.editor.preview(&self.runtime, now_ms());
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
        for (kind, target) in [("event", 0), ("application", 1), ("session", 2), ("all", 3)] {
            let rows = editor::activity_rows(
                &self.editor.config,
                &self.runtime,
                kind,
                "",
                target == 3,
                now_ms(),
            )
            .into_iter()
            .map(|row| ActivityRow {
                id: row.id.into(),
                kind: row.kind.into(),
                title: row.title.into(),
                subtitle: row.subtitle.into(),
                time: row.time.into(),
                detail: row.detail.into(),
                locked: row.locked,
            })
            .collect();
            match target {
                0 => ui.set_events(sync_model(ui.get_events(), rows)),
                1 => ui.set_totals(sync_model(ui.get_totals(), rows)),
                2 => ui.set_sessions(sync_model(ui.get_sessions(), rows)),
                _ => ui.set_archived(sync_model(ui.get_archived(), rows)),
            }
        }
        self.refresh_artwork(ui);
    }
    fn refresh_artwork(&mut self, ui: &MainWindow) {
        let now = std::time::Instant::now();
        self.artwork_retry_after
            .retain(|_, deadline| *deadline > now);
        self.artwork_requests
            .retain(|_, request| !request.task.is_finished());
        for (url, is_large) in [
            (self.editor.draft.large_image.clone(), true),
            (self.editor.draft.small_image.clone(), false),
        ] {
            let previous = if is_large {
                &mut self.large_uri
            } else {
                &mut self.small_uri
            };
            if *previous != url {
                *previous = url.clone();
                if let Some(previous) = self.artwork_requests.remove(&is_large) {
                    previous.task.abort();
                }
                let cached = self.artwork.get(&url).cloned();
                let loaded = cached.is_some();
                let image = cached.unwrap_or_default();
                if is_large {
                    ui.set_preview_large(image);
                    ui.set_has_preview_large(loaded);
                } else {
                    ui.set_preview_small(image);
                    ui.set_has_preview_small(loaded);
                }
            }
            if !url.starts_with("https://")
                || self.artwork.contains_key(&url)
                || self.artwork_retry_after.contains_key(&url)
                || self
                    .artwork_requests
                    .get(&is_large)
                    .is_some_and(|request| request.url == url)
            {
                continue;
            }
            let tx = self.updates.clone();
            let requested_url = url.clone();
            let task = self.executor.spawn(async move {
                // Wait for typing to settle. Changing the field aborts this
                // slot's task, including an HTTP request already in progress.
                tokio::time::sleep(Duration::from_millis(350)).await;
                let result = artwork::fetch(&requested_url).await;
                let _ = tx.send(Update::Artwork(is_large, requested_url, result));
            });
            self.artwork_requests
                .insert(is_large, ArtworkRequest { url, task });
        }
    }

    fn receive_artwork(
        &mut self,
        ui: &MainWindow,
        _is_large: bool,
        url: String,
        result: Result<artwork::Pixels, String>,
    ) {
        match result {
            Ok(pixels) => {
                self.artwork_retry_after.remove(&url);
                let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &pixels.rgba,
                    pixels.width,
                    pixels.height,
                );
                let image = slint::Image::from_rgba8(buffer);
                if url == self.large_uri {
                    ui.set_preview_large(image.clone());
                    ui.set_has_preview_large(true);
                }
                if url == self.small_uri {
                    ui.set_preview_small(image.clone());
                    ui.set_has_preview_small(true);
                }
                // Keep the currently displayed images when evicting a cached
                // image. Completed requests are removed by refresh_artwork,
                // so returning to an evicted URL can fetch it again.
                if self.artwork.len() >= 32 && !self.artwork.contains_key(&url) {
                    let candidate = self
                        .artwork
                        .keys()
                        .find(|key| *key != &self.large_uri && *key != &self.small_uri)
                        .cloned();
                    if let Some(candidate) = candidate {
                        self.artwork.remove(&candidate);
                    }
                }
                self.artwork.insert(url, image);
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
    fn action(&mut self, ui: &MainWindow, action: &str, argument: &str) {
        if ui.get_busy() && !matches!(action, "dismiss-message" | "modal-cancel") {
            return;
        }
        let result: Result<(), String> = (|| {
            match action {
                "search" => ui.set_search(argument.into()),
                "edit-global-id" => ui.set_global_id_draft(argument.into()),
                "dismiss-message" => ui.set_message("".into()),
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
                    ui.set_modal("".into());
                    self.rule = None;
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
                "quit" => self.request_pending(ui, Pending::Close),
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
            Command::Stop => match backend.stop().await {
                Ok(()) => {
                    let _ = updates.send(Update::Saved {
                        config: Box::new(backend.config().await),
                        selected: None,
                    });
                    Ok("Presence stopped. Automatic mode is off.".into())
                }
                Err(error) => Err(error),
            },
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
                    "Activity updated.".into()
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
    if smoke_root.is_none() {
        native::place_initial_window(ui.window());
    }
    ui.set_global_id_draft(config.application_id.clone().into());
    ui.set_data_path(root.display().to_string().into());
    ui.set_message(warnings.join("\n").into());
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
        large_uri: String::new(),
        small_uri: String::new(),
    }));
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
    let weak = ui.as_weak();
    let handler = host.clone();
    ui.window().on_close_requested(move || {
        if let Some(ui) = weak.upgrade() {
            let mut host = handler.borrow_mut();
            if !host.shutting_down && !ui.get_busy() {
                host.request_pending(&ui, Pending::Close);
            }
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    let weak = ui.as_weak();
    let handler = host.clone();
    let timer = slint::Timer::default();
    let mut last_second = 0;
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
        instance.poll_activation();
        let Some(ui) = weak.upgrade() else { return; };
        let mut host = handler.borrow_mut();
        let mut changed = false;
        while let Ok(update) = receiver.try_recv() {
            match update {
                Update::Runtime(runtime) => { host.runtime = *runtime; changed = true; }
                Update::Saved { config, selected } => {
                    let old_id = host.editor.config.application_id.clone();
                    host.editor.accept(*config, selected.as_deref());
                    if old_id != host.editor.config.application_id
                        || ui.get_global_id_draft().trim() == host.editor.config.application_id
                    {
                        ui.set_global_id_draft(host.editor.config.application_id.clone().into());
                    }
                    if selected.is_some() { host.refresh_form(&ui); }
                    changed = true;
                }
                Update::Finished(result) => {
                    ui.set_busy(false);
                    ui.set_scanning(false);
                    match result {
                        Ok(message) => {
                            if !message.is_empty() { ui.set_message(message.into()); }
                            if host.resume_after_save {
                                host.resume_after_save = false;
                                host.continue_pending(&ui);
                            }
                        }
                        Err(error) => { host.resume_after_save = false; host.error(&ui, error); }
                    }
                    changed = true;
                }
                Update::Shutdown(result) => { if let Err(error) = result { host.shutting_down = false; ui.set_busy(false); host.error(&ui, format!("Could not save application time: {error}. Close again to retry.")); } else { let _ = slint::quit_event_loop(); } }
                Update::Artwork(is_large, url, result) => { host.receive_artwork(&ui, is_large, url, result); changed = true; }
            }
        } let second = now_ms() / 1000; if changed || second != last_second { last_second = second; host.refresh(&ui); }
    });
    let smoke_run = smoke_root.map(|root| smoke::schedule(&ui, host.clone(), root));
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
