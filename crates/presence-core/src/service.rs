//! Presence orchestration, including activity history and recoverable persistence.
//!
//! The desktop owns scheduling and holds this service behind a Tokio mutex. Every
//! operation therefore runs in the same serialized order as Electron's queue.
use crate::detection::detect_apps;
use crate::engine::{
    application_id_for, build_activity, match_rule, metadata_for, playback_duration_seconds,
    validate_profile,
};
use crate::model::{
    ApplicationTime, ArchiveData, ArchivedActivity, Config, DetectedApp, LogEntry, Metadata,
    Recording, RuntimeState, UsageData, UsageSession,
};
use crate::rpc::{DisconnectEvent, DiscordRpc};
use serde_json::Value;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use uuid::Uuid;

pub type SaveUsage = Arc<dyn Fn(&UsageData) -> Result<(), String> + Send + Sync>;
pub type SaveArchive = Arc<dyn Fn(&ArchiveData) -> Result<(), String> + Send + Sync>;
pub type OnChange = Arc<dyn Fn(RuntimeState) + Send + Sync>;
pub type Detector = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<Vec<DetectedApp>, String>> + Send>>
        + Send
        + Sync,
>;

/// A small RPC seam keeps orchestration tests independent of Discord and pipes.
pub trait RpcClient: Send {
    fn connect(
        &mut self,
        application_id: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;
    fn set_activity(&mut self, activity: Value) -> impl Future<Output = Result<(), String>> + Send;
    fn disconnect(&mut self);
    fn client_id(&self) -> &str;
    fn is_connected(&self) -> bool;
    fn disconnected_at(&self) -> Option<i64> {
        None
    }
}

impl RpcClient for DiscordRpc {
    async fn connect(&mut self, application_id: &str) -> Result<String, String> {
        DiscordRpc::connect(self, application_id).await
    }
    async fn set_activity(&mut self, activity: Value) -> Result<(), String> {
        DiscordRpc::set_activity(self, activity).await
    }
    fn disconnect(&mut self) {
        DiscordRpc::disconnect(self);
    }
    fn client_id(&self) -> &str {
        DiscordRpc::client_id(self)
    }
    fn is_connected(&self) -> bool {
        DiscordRpc::is_connected(self)
    }
    fn disconnected_at(&self) -> Option<i64> {
        DiscordRpc::disconnected_at(self)
    }
}

pub struct Service<R: RpcClient = DiscordRpc> {
    pub config: Config,
    pub state: RuntimeState,
    rpc: R,
    last_payload: String,
    source_pid: Option<u32>,
    playback_duration: Option<i64>,
    usage_dirty: bool,
    archive_dirty: bool,
    usage_total_ids: BTreeMap<String, String>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    detect: Detector,
    save_usage: SaveUsage,
    save_archive: SaveArchive,
    on_change: Option<OnChange>,
}

impl Service<DiscordRpc> {
    pub fn new(config: Config, usage: UsageData, archive: ArchiveData) -> Self {
        Self::with_rpc(config, usage, archive, DiscordRpc::new())
    }

    pub fn subscribe_disconnects(&self) -> broadcast::Receiver<DisconnectEvent> {
        self.rpc.subscribe_disconnects()
    }

    pub fn connection_id(&self) -> u64 {
        self.rpc.connection_id()
    }
}

impl<R: RpcClient> Service<R> {
    pub fn with_rpc(config: Config, usage: UsageData, archive: ArchiveData, rpc: R) -> Self {
        let mut state = RuntimeState {
            usage_totals: usage.totals,
            archive: archive.entries,
            ..RuntimeState::default()
        };
        let mut usage_total_ids = BTreeMap::new();
        let mut usage_dirty = false;
        for id in state.usage_totals.keys().cloned().collect::<Vec<_>>() {
            let total_id = usage
                .total_ids
                .as_ref()
                .and_then(|ids| ids.get(&id))
                .cloned()
                .unwrap_or_else(|| format!("legacy:{id}"));
            if state.archive.iter().any(|item| {
                item.kind == "application"
                    && !item.tracks_application_time()
                    && string_field(&item.entry, "ruleId") == Some(id.as_str())
                    && string_field(&item.entry, "id") == Some(total_id.as_str())
            }) {
                // The archive's first write can survive an interrupted usage write.
                state.usage_totals.remove(&id);
                usage_dirty = true;
            } else {
                usage_total_ids.insert(id, total_id);
            }
        }
        let session_count = usage.sessions.len();
        state.usage_sessions = usage
            .sessions
            .into_iter()
            .filter(|session| {
                !state.archive.iter().any(|item| {
                    item.kind == "session"
                        && string_field(&item.entry, "id") == Some(session.id.as_str())
                })
            })
            .collect();
        usage_dirty |= session_count != state.usage_sessions.len();
        let mut service = Self {
            config,
            state,
            rpc,
            last_payload: String::new(),
            source_pid: None,
            playback_duration: None,
            usage_dirty,
            archive_dirty: false,
            usage_total_ids,
            now: Arc::new(unix_millis),
            detect: Arc::new(|| Box::pin(detect_apps())),
            save_usage: Arc::new(|_| Ok(())),
            save_archive: Arc::new(|_| Ok(())),
            on_change: None,
        };
        // Usage is saved before linked archive updates. Recover an interrupted
        // archive write from the latest checkpoint without reviving deleted totals.
        for id in service
            .state
            .usage_totals
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            service.sync_archived_total(&id);
        }
        for item in &mut service.state.archive {
            if item.tracks_application_time()
                && string_field(&item.entry, "ruleId")
                    .and_then(|rule| service.usage_total_ids.get(rule))
                    .map(String::as_str)
                    != string_field(&item.entry, "id")
            {
                item.extra
                    .insert("tracksApplicationTime".into(), false.into());
                service.archive_dirty = true;
            }
        }
        service
    }

    pub fn set_persistence(&mut self, save_usage: SaveUsage, save_archive: SaveArchive) {
        self.save_usage = save_usage;
        self.save_archive = save_archive;
    }

    pub fn set_on_change(&mut self, callback: OnChange) {
        self.on_change = Some(callback);
    }

    pub fn set_detector(&mut self, detector: Detector) {
        self.detect = detector;
    }

    pub fn set_clock(&mut self, now: Arc<dyn Fn() -> i64 + Send + Sync>) {
        self.now = now;
    }

    pub fn runtime(&self) -> RuntimeState {
        self.state.clone()
    }

    pub fn usage_data(&self) -> UsageData {
        UsageData {
            totals: self.state.usage_totals.clone(),
            sessions: self.state.usage_sessions.clone(),
            total_ids: Some(self.usage_total_ids.clone()),
        }
    }

    pub fn archive_data(&self) -> ArchiveData {
        ArchiveData {
            entries: self.state.archive.clone(),
        }
    }

    pub fn rpc_is_connected(&self) -> bool {
        self.rpc.is_connected()
    }

    fn checkpoint_usage_at(&mut self, stop: bool, at: i64) {
        let Some(recording) = self.state.recording.clone() else {
            return;
        };
        // The RPC reader runs independently of the service mutex. A disconnect
        // can happen while a scan or command holds that mutex, before its event
        // listener can enter. Clamp to the transport's actual close timestamp.
        let disconnected_at = self.rpc.disconnected_at();
        let at = disconnected_at.map(|closed| at.min(closed)).unwrap_or(at);
        let stop = stop || disconnected_at.is_some();
        let now = recording.since.max(at);
        let duration = now.saturating_sub(recording.since);
        if duration > 0 {
            let total = self
                .state
                .usage_totals
                .entry(recording.rule_id.clone())
                .or_default();
            *total = total.saturating_add(duration);
            self.usage_dirty = true;
            if let Some(session) = self
                .state
                .usage_sessions
                .iter_mut()
                .find(|session| session.id == recording.session_id)
            {
                session.duration_ms = session.duration_ms.saturating_add(duration);
                session.ended_at = session.ended_at.max(now);
            }
        }
        if stop {
            self.sync_archived_total(&recording.rule_id);
        }
        self.state.recording = if stop {
            None
        } else {
            Some(Recording {
                since: now,
                ..recording
            })
        };
    }

    fn checkpoint_usage(&mut self, stop: bool) {
        self.checkpoint_usage_at(stop, (self.now)());
    }

    fn sync_recording(&mut self) {
        let rule = self
            .config
            .rules
            .iter()
            .find(|rule| Some(rule.id.as_str()) == self.state.active_rule_id.as_deref())
            .cloned();
        let rule_id = rule.as_ref().filter(|rule| {
            self.config.record_time
                && self.config.automatic
                && self.state.connection == "connected"
                && self.rpc.is_connected()
                && self.rpc.disconnected_at().is_none()
                && self.state.mode.as_deref() == Some("automatic")
                && rule.enabled
                && Some(rule.profile_id.as_str()) == self.state.active_profile_id.as_deref()
        });
        if self.state.recording.as_ref().map(|r| r.rule_id.as_str())
            == rule_id.map(|r| r.id.as_str())
        {
            return;
        }
        self.checkpoint_usage(true);
        if let Some(rule) = rule_id {
            if !self.state.usage_totals.contains_key(&rule.id) {
                self.state.usage_totals.insert(rule.id.clone(), 0);
                self.usage_total_ids
                    .insert(rule.id.clone(), Uuid::new_v4().to_string());
            }
            let since = (self.now)();
            let session_id = Uuid::new_v4().to_string();
            self.state.usage_sessions.insert(
                0,
                UsageSession {
                    id: session_id.clone(),
                    rule_id: rule.id.clone(),
                    name: if rule.name.is_empty() {
                        rule.process.clone()
                    } else {
                        rule.name.clone()
                    },
                    process: rule.process.clone(),
                    started_at: since,
                    ended_at: since,
                    duration_ms: 0,
                    extra: Default::default(),
                },
            );
            self.state.usage_sessions.truncate(500);
            self.state.recording = Some(Recording {
                session_id,
                rule_id: rule.id.clone(),
                since,
            });
            self.usage_dirty = true;
        }
    }

    fn persist_usage(&mut self, strict: bool) -> Result<(), String> {
        if !self.usage_dirty {
            return self.persist_archive(false);
        }
        match (self.save_usage)(&self.usage_data()) {
            Ok(()) => {
                self.usage_dirty = false;
                self.persist_archive(false)
            }
            Err(error) => {
                if strict {
                    return Err(error);
                }
                let message = "Application time could not be saved. It will be retried.";
                if self.state.logs.first().map(|item| item.message.as_str()) != Some(message) {
                    self.log("error", message);
                }
                Ok(())
            }
        }
    }

    pub fn flush_usage(&mut self) -> Result<(), String> {
        self.persist_usage(true)?;
        self.persist_archive(true)
    }

    fn persist_archive(&mut self, strict: bool) -> Result<(), String> {
        if !self.archive_dirty {
            return Ok(());
        }
        match (self.save_archive)(&self.archive_data()) {
            Ok(()) => {
                self.archive_dirty = false;
                Ok(())
            }
            Err(error) => {
                if strict {
                    return Err(error);
                }
                let message = "Archived application time could not be updated. It will be retried.";
                if self.state.logs.first().map(|entry| entry.message.as_str()) != Some(message) {
                    self.log("error", message);
                }
                Ok(())
            }
        }
    }

    fn sync_archived_total(&mut self, rule_id: &str) {
        let Some(total_id) = self.usage_total_ids.get(rule_id) else {
            return;
        };
        let Some(duration) = self.state.usage_totals.get(rule_id) else {
            return;
        };
        for item in &mut self.state.archive {
            if item.tracks_application_time()
                && string_field(&item.entry, "id") == Some(total_id.as_str())
                && string_field(&item.entry, "ruleId") == Some(rule_id)
                && item.entry["durationMs"] != *duration
            {
                item.entry["durationMs"] = (*duration).into();
                self.archive_dirty = true;
            }
        }
    }

    fn validate_activity_reference(kind: &str, id: &str) -> Result<(), String> {
        if !matches!(kind, "event" | "session" | "application") || id.is_empty() {
            Err("Invalid activity entry.".into())
        } else {
            Ok(())
        }
    }

    fn application_time(&self, rule_id: &str) -> Result<ApplicationTime, String> {
        let duration = *self
            .state
            .usage_totals
            .get(rule_id)
            .ok_or("This application time no longer exists.")?;
        let rule = self.config.rules.iter().find(|rule| rule.id == rule_id);
        let session = self
            .state
            .usage_sessions
            .iter()
            .find(|session| session.rule_id == rule_id)
            .cloned()
            .or_else(|| {
                self.state
                    .archive
                    .iter()
                    .find(|item| {
                        item.kind == "session"
                            && string_field(&item.entry, "ruleId") == Some(rule_id)
                    })
                    .and_then(|item| serde_json::from_value(item.entry.clone()).ok())
            });
        let name = rule
            .map(|r| r.name.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| rule.map(|r| r.process.as_str()).filter(|s| !s.is_empty()))
            .or_else(|| session.as_ref().map(|s| s.name.as_str()))
            .unwrap_or("Removed application")
            .to_owned();
        let process = rule
            .map(|r| r.process.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| session.as_ref().map(|s| s.process.as_str()))
            .unwrap_or("")
            .to_owned();
        Ok(ApplicationTime {
            id: self
                .usage_total_ids
                .get(rule_id)
                .cloned()
                .unwrap_or_else(|| format!("legacy:{rule_id}")),
            rule_id: rule_id.into(),
            name,
            process,
            duration_ms: duration,
        })
    }

    pub async fn archive_activity(&mut self, kind: &str, id: &str) -> Result<RuntimeState, String> {
        Self::validate_activity_reference(kind, id)?;
        if self.archive_dirty {
            self.persist_usage(true)?;
        }
        if kind == "application" {
            self.application_time(id)?;
            self.checkpoint_usage(false);
            // Save the baseline first so the linked archive can never outrun
            // its recoverable application total, including during live recording.
            self.persist_usage(true)?;
        }
        if kind == "session"
            && self.state.recording.as_ref().map(|r| r.session_id.as_str()) == Some(id)
        {
            return Err("Finish the session before archiving it.".into());
        }
        let entry = match kind {
            "application" => Some(serde_json::to_value(self.application_time(id)?).unwrap()),
            "event" => self
                .state
                .logs
                .iter()
                .find(|item| item.id == id)
                .map(|item| serde_json::to_value(item).unwrap()),
            "session" => self
                .state
                .usage_sessions
                .iter()
                .find(|item| item.id == id)
                .map(|item| serde_json::to_value(item).unwrap()),
            _ => None,
        }
        .ok_or("This activity entry no longer exists.")?;
        let existing = self.state.archive.iter().position(|item| {
            item.kind == kind && string_field(&item.entry, "id") == string_field(&entry, "id")
        });
        if existing.is_some() && kind != "application" {
            return Err("This entry is already archived.".into());
        }
        let mut entries = self.state.archive.clone();
        if let Some(index) = existing {
            if entries[index].tracks_application_time() {
                self.publish();
                return Ok(self.runtime());
            }
            entries[index].entry = entry;
            entries[index]
                .extra
                .insert("tracksApplicationTime".into(), true.into());
        } else {
            let extra = if kind == "application" {
                serde_json::Map::from_iter([("tracksApplicationTime".into(), true.into())])
            } else {
                Default::default()
            };
            entries.insert(
                0,
                ArchivedActivity {
                    kind: kind.into(),
                    entry,
                    archived_at: (self.now)(),
                    extra,
                },
            );
        }
        // Events and sessions move only after their permanent copy is saved.
        // Application totals remain in usage and follow the same generation.
        (self.save_archive)(&ArchiveData {
            entries: entries.clone(),
        })?;
        self.state.archive = entries;
        self.archive_dirty = false;
        if kind == "event" {
            self.state.logs.retain(|item| item.id != id);
        } else if kind == "session" {
            self.checkpoint_usage(false);
            self.state.usage_sessions.retain(|item| item.id != id);
            self.usage_dirty = true;
            self.persist_usage(false)?;
        }
        self.publish();
        Ok(self.runtime())
    }

    pub async fn delete_activity(&mut self, kind: &str, id: &str) -> Result<RuntimeState, String> {
        Self::validate_activity_reference(kind, id)?;
        if kind == "session"
            && self.state.recording.as_ref().map(|r| r.session_id.as_str()) == Some(id)
        {
            return Err("Finish the session before deleting it.".into());
        }
        match kind {
            "event" => {
                if !self.state.logs.iter().any(|item| item.id == id) {
                    return Err("This activity entry no longer exists.".into());
                }
                self.state.logs.retain(|item| item.id != id);
            }
            "application" => {
                if self.state.recording.as_ref().map(|r| r.rule_id.as_str()) == Some(id) {
                    return Err(
                        "Pause time recording before deleting this application’s time.".into(),
                    );
                }
                let entry = self.application_time(id)?;
                self.checkpoint_usage(false);
                self.state.usage_totals.remove(id);
                self.usage_total_ids.remove(id);
                self.usage_dirty = true;
                if let Err(error) = self.persist_usage(true) {
                    self.state.usage_totals.insert(id.into(), entry.duration_ms);
                    self.usage_total_ids.insert(id.into(), entry.id);
                    self.usage_dirty = true;
                    self.publish();
                    return Err(error);
                }
                for item in &mut self.state.archive {
                    if item.tracks_application_time()
                        && string_field(&item.entry, "id") == Some(entry.id.as_str())
                        && string_field(&item.entry, "ruleId") == Some(id)
                    {
                        item.extra
                            .insert("tracksApplicationTime".into(), false.into());
                        self.archive_dirty = true;
                    }
                }
                self.persist_archive(false)?;
            }
            "session" => {
                if !self.state.usage_sessions.iter().any(|item| item.id == id) {
                    return Err("This activity entry no longer exists.".into());
                }
                self.checkpoint_usage(false);
                let sessions = self.state.usage_sessions.clone();
                self.state.usage_sessions.retain(|item| item.id != id);
                self.usage_dirty = true;
                if let Err(error) = self.persist_usage(true) {
                    self.state.usage_sessions = sessions;
                    self.publish();
                    return Err(error);
                }
            }
            _ => unreachable!(),
        }
        self.publish();
        Ok(self.runtime())
    }

    pub async fn delete_archived_activity(
        &mut self,
        kind: &str,
        id: &str,
    ) -> Result<RuntimeState, String> {
        Self::validate_activity_reference(kind, id)?;
        if !self
            .state
            .archive
            .iter()
            .any(|item| item.kind == kind && string_field(&item.entry, "id") == Some(id))
        {
            return Err("This archived entry no longer exists.".into());
        }
        if self.archive_dirty || matches!(kind, "session" | "application") {
            // Do not remove the only permanent copy until recent-history cleanup
            // has reached disk successfully.
            self.persist_usage(true)?;
        }
        let entries = self
            .state
            .archive
            .iter()
            .filter(|item| item.kind != kind || string_field(&item.entry, "id") != Some(id))
            .cloned()
            .collect::<Vec<_>>();
        (self.save_archive)(&ArchiveData {
            entries: entries.clone(),
        })?;
        self.state.archive = entries;
        self.archive_dirty = false;
        self.publish();
        Ok(self.runtime())
    }

    fn publish(&self) {
        if let Some(callback) = &self.on_change {
            callback(self.runtime());
        }
    }

    pub fn log(&mut self, level: &str, message: &str) {
        self.state.logs.insert(
            0,
            LogEntry {
                id: Uuid::new_v4().to_string(),
                at: unix_millis(),
                level: level.into(),
                message: message.into(),
            },
        );
        self.state.logs.truncate(100);
        self.publish();
    }

    /// Called by the desktop's disconnect listener with the read loop's timestamp.
    pub fn on_disconnect(&mut self, at: i64) {
        self.checkpoint_usage_at(true, at);
        let _ = self.persist_usage(false);
        self.state.connection = "disconnected".into();
        self.state.user = None;
        self.last_payload.clear();
        let message = if self.state.mode.as_deref() == Some("automatic") {
            "Discord disconnected. Automatic mode will retry."
        } else {
            "Discord disconnected. Go live again to reconnect."
        };
        self.state.error = Some(message.into());
        self.log("error", message);
    }

    pub async fn scan(&mut self) -> Result<Vec<DetectedApp>, String> {
        let apps = (self.detect)().await?;
        self.state.apps = apps.clone();
        self.publish();
        Ok(apps)
    }

    pub async fn activate(&mut self, id: &str) -> Result<(), String> {
        self.set_presence(id, None, "manual", None, None).await
    }

    async fn set_presence(
        &mut self,
        id: &str,
        metadata: Option<Metadata>,
        mode: &str,
        pid: Option<u32>,
        rule_id: Option<String>,
    ) -> Result<(), String> {
        let profile = self
            .config
            .profiles
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or("This presence no longer exists.")?;
        validate_profile(&profile, true, &self.config.application_id)?;
        let application_id = application_id_for(&profile, &self.config.application_id).to_owned();
        let same_session = self.state.active_profile_id.as_deref() == Some(id)
            && self.source_pid == pid
            && self.state.mode.as_deref() == Some(mode)
            && self.state.active_rule_id == rule_id;
        if !same_session {
            self.checkpoint_usage(true);
        }
        let duration = playback_duration_seconds(&profile);
        let since = if same_session && duration == self.playback_duration {
            self.state.since.unwrap_or_else(|| (self.now)())
        } else {
            (self.now)()
        };
        let result = async {
            if self.state.connection != "connected"
                || !self.rpc.is_connected()
                || self.rpc.disconnected_at().is_some()
                || self.rpc.client_id() != application_id
            {
                self.checkpoint_usage(true);
                self.state.connection = "connecting".into();
                self.publish();
                self.state.user = Some(self.rpc.connect(&application_id).await?);
                self.state.connection = "connected".into();
                self.last_payload.clear();
            }
            let activity = build_activity(&profile, metadata.as_ref(), since);
            let payload = activity.to_string();
            if payload != self.last_payload {
                self.rpc.set_activity(activity).await?;
                self.last_payload = payload;
            }
            let changed = self.state.active_profile_id.as_deref() != Some(id)
                || self.state.mode.as_deref() != Some(mode)
                || self.source_pid != pid
                || self.state.active_rule_id != rule_id;
            self.state.active_profile_id = Some(id.into());
            self.state.active_rule_id = rule_id;
            self.state.mode = Some(mode.into());
            self.state.metadata = metadata;
            self.state.since = Some(since);
            self.playback_duration = duration;
            self.source_pid = pid;
            self.state.error = None;
            self.sync_recording();
            if changed {
                let process = self
                    .state
                    .metadata
                    .as_ref()
                    .map(|m| format!(" · {}", m.process))
                    .unwrap_or_default();
                self.log("success", &format!("{} is live{process}.", profile.name));
            } else {
                self.publish();
            }
            Ok::<(), String>(())
        }
        .await;
        if let Err(error) = &result {
            self.checkpoint_usage(true);
            self.state.error = Some(error.clone());
            self.rpc.disconnect();
            self.state.connection = "disconnected".into();
            self.state.user = None;
            self.last_payload.clear();
            self.log("error", error);
        }
        let _ = self.persist_usage(false);
        result
    }

    pub async fn stop(&mut self) -> Result<(), String> {
        self.checkpoint_usage(true);
        let result = if self.state.connection == "connected" {
            self.rpc.set_activity(Value::Null).await
        } else {
            Ok(())
        };
        self.rpc.disconnect();
        self.reset();
        self.log("info", "Presence stopped.");
        let _ = self.persist_usage(false);
        result
    }

    fn reset(&mut self) {
        self.checkpoint_usage(true);
        self.state.connection = "disconnected".into();
        self.state.active_profile_id = None;
        self.state.active_rule_id = None;
        self.state.mode = None;
        self.state.metadata = None;
        self.state.since = None;
        self.state.user = None;
        self.state.error = None;
        self.last_payload.clear();
        self.source_pid = None;
        self.playback_duration = None;
    }

    pub async fn tick(&mut self) {
        self.checkpoint_usage(false);
        if !self.config.automatic || self.state.mode.as_deref() == Some("manual") {
            self.sync_recording();
            let _ = self.persist_usage(false);
            return;
        }
        let result = async {
            let apps = self.scan().await?;
            let matched = match_rule(&self.config.rules, &apps)
                .map(|(rule, app)| (rule.clone(), app.clone()));
            if let Some((rule, app)) = matched {
                self.set_presence(
                    &rule.profile_id,
                    Some(metadata_for(&app)),
                    "automatic",
                    Some(app.pid),
                    Some(rule.id),
                )
                .await?;
            } else if self.state.mode.as_deref() == Some("automatic") {
                self.checkpoint_usage(true);
                if self.state.connection == "connected" {
                    self.rpc.set_activity(Value::Null).await?;
                }
                self.rpc.disconnect();
                self.reset();
                self.log("info", "No matching application. Presence cleared.");
            }
            Ok::<(), String>(())
        }
        .await;
        if let Err(error) = result {
            self.checkpoint_usage(true);
            if self.state.mode.as_deref() == Some("automatic") {
                self.rpc.disconnect();
                self.reset();
            }
            self.state.error = Some(error.clone());
            self.publish();
            if self.state.logs.first().map(|entry| entry.message.as_str()) != Some(error.as_str()) {
                self.log("error", &error);
            }
        }
        let _ = self.persist_usage(false);
    }

    pub async fn set_config(&mut self, config: Config) -> Result<(), String> {
        self.checkpoint_usage(false);
        self.config = config;
        self.sync_recording();
        if let Some(id) = self.state.active_profile_id.clone() {
            let active = self.config.profiles.iter().find(|p| p.id == id);
            let invalid = active
                .map(|p| {
                    let app_id = application_id_for(p, &self.config.application_id);
                    !(17..=20).contains(&app_id.len())
                        || !app_id.bytes().all(|b| b.is_ascii_digit())
                })
                .unwrap_or(true);
            if invalid
                || (self.state.mode.as_deref() == Some("automatic") && !self.config.automatic)
            {
                let result = if self.state.connection == "connected" {
                    self.rpc.set_activity(Value::Null).await
                } else {
                    Ok(())
                };
                self.rpc.disconnect();
                self.reset();
                self.publish();
                let _ = self.persist_usage(false);
                result?;
            } else if self.state.connection == "connected" {
                self.set_presence(
                    &id,
                    self.state.metadata.clone(),
                    &self.state.mode.clone().unwrap_or_else(|| "manual".into()),
                    self.source_pid,
                    self.state.active_rule_id.clone(),
                )
                .await?;
            }
        }
        self.publish();
        let _ = self.persist_usage(false);
        Ok(())
    }

    pub async fn dispose(&mut self) {
        self.checkpoint_usage(true);
        self.rpc.disconnect();
        let _ = self.persist_usage(false);
    }
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{validate_archive_data, validate_usage_data};
    use crate::model::{default_config, AppRule};
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct RpcState {
        fail: bool,
        fail_connect: bool,
        calls: Vec<Value>,
        connected: bool,
        disconnected_at: Option<i64>,
    }

    struct MockRpc {
        state: Arc<Mutex<RpcState>>,
        client_id: String,
    }

    impl RpcClient for MockRpc {
        async fn connect(&mut self, id: &str) -> Result<String, String> {
            let mut state = self.state.lock().unwrap();
            if state.fail_connect {
                return Err("Connection rejected".into());
            }
            state.connected = true;
            state.disconnected_at = None;
            self.client_id = id.into();
            Ok("Test".into())
        }
        async fn set_activity(&mut self, activity: Value) -> Result<(), String> {
            let mut state = self.state.lock().unwrap();
            if state.fail {
                return Err("Rejected".into());
            }
            state.calls.push(activity);
            Ok(())
        }
        fn disconnect(&mut self) {
            let mut state = self.state.lock().unwrap();
            state.connected = false;
            state.disconnected_at = None;
            self.client_id.clear();
        }
        fn client_id(&self) -> &str {
            &self.client_id
        }
        fn is_connected(&self) -> bool {
            self.state.lock().unwrap().connected
        }
        fn disconnected_at(&self) -> Option<i64> {
            self.state.lock().unwrap().disconnected_at
        }
    }

    struct Fixture {
        service: Service<MockRpc>,
        rpc: Arc<Mutex<RpcState>>,
        apps: Arc<Mutex<Result<Vec<DetectedApp>, String>>>,
        now: Arc<AtomicI64>,
        usage: Arc<Mutex<UsageData>>,
        archive: Arc<Mutex<ArchiveData>>,
        fail_usage: Arc<AtomicBool>,
        fail_archive: Arc<AtomicBool>,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_data(UsageData::default(), ArchiveData::default())
        }

        fn with_data(usage: UsageData, archive: ArchiveData) -> Self {
            let mut config = default_config();
            config.profiles[0].application_id = "123456789012345678".into();
            config.profiles[0].details = "{{title}}".into();
            config.rules = vec![AppRule {
                id: "r1".into(),
                name: "Editor".into(),
                process: "Code.exe".into(),
                title_contains: "".into(),
                profile_id: config.profiles[0].id.clone(),
                enabled: true,
                extra: Default::default(),
            }];
            let rpc = Arc::new(Mutex::new(RpcState::default()));
            let mut service = Service::with_rpc(
                config,
                usage.clone(),
                archive.clone(),
                MockRpc {
                    state: rpc.clone(),
                    client_id: "".into(),
                },
            );
            let apps = Arc::new(Mutex::new(Ok(vec![DetectedApp {
                pid: 100,
                process: "Code.exe".into(),
                title: "main.ts".into(),
                name: Some("VS Code".into()),
            }])));
            let source = apps.clone();
            service.set_detector(Arc::new(move || {
                let result = source.lock().unwrap().clone();
                Box::pin(async move { result })
            }));
            let now = Arc::new(AtomicI64::new(1_000));
            let clock = now.clone();
            service.set_clock(Arc::new(move || clock.load(Ordering::SeqCst)));
            let usage = Arc::new(Mutex::new(usage));
            let archive = Arc::new(Mutex::new(archive));
            let fail_usage = Arc::new(AtomicBool::new(false));
            let fail_archive = Arc::new(AtomicBool::new(false));
            let usage_target = usage.clone();
            let archive_target = archive.clone();
            let usage_failure = fail_usage.clone();
            let archive_failure = fail_archive.clone();
            service.set_persistence(
                Arc::new(move |usage| {
                    if usage_failure.load(Ordering::SeqCst) {
                        return Err("Usage unavailable".into());
                    }
                    *usage_target.lock().unwrap() = usage.clone();
                    Ok(())
                }),
                Arc::new(move |archive| {
                    if archive_failure.load(Ordering::SeqCst) {
                        return Err("Archive unavailable".into());
                    }
                    *archive_target.lock().unwrap() = archive.clone();
                    Ok(())
                }),
            );
            Self {
                service,
                rpc,
                apps,
                now,
                usage,
                archive,
                fail_usage,
                fail_archive,
            }
        }

        fn automatic(&mut self, record: bool) {
            self.service.config.automatic = true;
            self.service.config.record_time = record;
        }
        fn advance(&self, ms: i64) {
            self.now.fetch_add(ms, Ordering::SeqCst);
        }
        fn calls(&self) -> Vec<Value> {
            self.rpc.lock().unwrap().calls.clone()
        }
        fn set_apps(&self, apps: Vec<DetectedApp>) {
            *self.apps.lock().unwrap() = Ok(apps);
        }
        async fn activate(&mut self) {
            let id = self.service.config.profiles[0].id.clone();
            self.service.activate(&id).await.unwrap();
        }
        async fn record(&mut self, enabled: bool) {
            let mut config = self.service.config.clone();
            config.record_time = enabled;
            self.service.set_config(config).await.unwrap();
        }
        fn saved_usage(&self) -> UsageData {
            self.usage.lock().unwrap().clone()
        }
        fn saved_archive(&self) -> ArchiveData {
            self.archive.lock().unwrap().clone()
        }
    }

    #[tokio::test]
    async fn automatic_is_opt_in_and_title_changes_keep_timer_and_coalesce_payloads() {
        let mut f = Fixture::new();
        f.service.tick().await;
        assert!(f.calls().is_empty());
        f.automatic(false);
        f.service.tick().await;
        let since = f.service.state.since;
        assert_eq!(f.service.state.mode.as_deref(), Some("automatic"));
        assert_eq!(f.service.state.metadata.as_ref().unwrap().title, "main.ts");
        f.advance(15_000);
        f.service.tick().await;
        assert_eq!(f.calls().len(), 1);
        f.apps.lock().unwrap().as_mut().unwrap()[0].title = "README.md".into();
        f.service.tick().await;
        assert_eq!(f.calls().len(), 2);
        assert_eq!(f.service.state.since, since);
        assert_eq!(f.calls().last().unwrap()["details"], "README.md");
    }

    #[tokio::test]
    async fn manual_takes_priority_and_closing_matching_apps_clears_automatic() {
        let mut f = Fixture::new();
        f.automatic(false);
        f.activate().await;
        f.set_apps(vec![]);
        f.service.tick().await;
        assert_eq!(f.service.state.mode.as_deref(), Some("manual"));
        assert_eq!(f.calls().len(), 1);
        f.service.stop().await.unwrap();
        f.set_apps(vec![DetectedApp {
            pid: 100,
            process: "Code.exe".into(),
            title: "main.ts".into(),
            name: None,
        }]);
        f.service.tick().await;
        f.set_apps(vec![]);
        f.service.tick().await;
        assert_eq!(f.calls().last(), Some(&Value::Null));
        assert_eq!(f.service.state.connection, "disconnected");
        assert_eq!(f.service.state.active_profile_id, None);
    }

    #[tokio::test]
    async fn global_application_id_inheritance_and_profile_override_reconnect() {
        let mut f = Fixture::new();
        f.service.config.application_id = "987654321098765432".into();
        f.service.config.profiles[0].application_id.clear();
        f.activate().await;
        assert_eq!(f.service.rpc.client_id(), "987654321098765432");
        let mut config = f.service.config.clone();
        config.application_id = "876543210987654321".into();
        f.service.set_config(config.clone()).await.unwrap();
        assert_eq!(f.service.rpc.client_id(), "876543210987654321");
        config.profiles[0].application_id = "123456789012345678".into();
        config.application_id.clear();
        f.service.set_config(config.clone()).await.unwrap();
        assert_eq!(f.service.rpc.client_id(), "123456789012345678");
        config.profiles[0].application_id.clear();
        f.service.set_config(config).await.unwrap();
        assert_eq!(f.service.state.active_profile_id, None);
        assert_eq!(f.calls().last(), Some(&Value::Null));
    }

    #[tokio::test]
    async fn failed_acknowledgements_do_not_report_live_and_stop_always_resets() {
        let mut f = Fixture::new();
        f.rpc.lock().unwrap().fail = true;
        let id = f.service.config.profiles[0].id.clone();
        assert_eq!(f.service.activate(&id).await.unwrap_err(), "Rejected");
        assert_eq!(f.service.state.connection, "disconnected");
        assert_eq!(f.service.state.active_profile_id, None);
        assert_eq!(f.service.state.logs[0].level, "error");
        f.rpc.lock().unwrap().fail = false;
        f.activate().await;
        f.rpc.lock().unwrap().fail = true;
        assert_eq!(f.service.stop().await.unwrap_err(), "Rejected");
        assert_eq!(f.service.state.connection, "disconnected");
        assert_eq!(f.service.state.active_profile_id, None);
    }

    #[tokio::test]
    async fn failed_clear_and_detection_failures_reset_automatic_presence() {
        let mut f = Fixture::new();
        f.automatic(true);
        f.service.tick().await;
        f.advance(12_000);
        f.rpc.lock().unwrap().fail = true;
        f.set_apps(vec![]);
        f.service.tick().await;
        assert_eq!(f.service.state.active_profile_id, None);
        assert_eq!(f.service.state.connection, "disconnected");
        assert_eq!(f.service.state.error.as_deref(), Some("Rejected"));
        assert_eq!(f.saved_usage().totals["r1"], 12_000);
        *f.apps.lock().unwrap() = Err("Application detection failed.".into());
        f.service.tick().await;
        assert_eq!(
            f.service.state.error.as_deref(),
            Some("Application detection failed.")
        );
        let count = f.service.state.logs.len();
        f.service.tick().await;
        assert_eq!(f.service.state.logs.len(), count);
    }

    #[tokio::test]
    async fn disabling_automatic_or_removing_active_profile_clears_discord() {
        let mut f = Fixture::new();
        f.automatic(true);
        f.service.tick().await;
        f.advance(8_000);
        let mut config = f.service.config.clone();
        config.automatic = false;
        f.service.set_config(config).await.unwrap();
        assert_eq!(f.service.state.active_profile_id, None);
        assert_eq!(f.calls().last(), Some(&Value::Null));
        assert_eq!(f.saved_usage().totals["r1"], 8_000);
        f.activate().await;
        let mut config = f.service.config.clone();
        config.profiles.remove(0);
        config.rules.clear();
        f.service.set_config(config).await.unwrap();
        assert_eq!(f.service.state.active_profile_id, None);
        assert_eq!(f.calls().last(), Some(&Value::Null));
    }

    #[tokio::test]
    async fn time_recording_is_opt_in_and_never_counts_manual_presence() {
        let mut f = Fixture::new();
        f.automatic(false);
        f.service.tick().await;
        f.advance(60_000);
        f.service.tick().await;
        assert!(f.service.state.usage_totals.is_empty());
        f.activate().await;
        f.record(true).await;
        f.advance(60_000);
        f.service.tick().await;
        assert!(f.service.state.recording.is_none());
        assert!(f.service.state.usage_sessions.is_empty());
    }

    #[tokio::test]
    async fn records_cumulative_sessions_across_titles_close_and_restart() {
        let mut f = Fixture::new();
        f.automatic(true);
        f.service.tick().await;
        let session = f.service.state.recording.clone().unwrap().session_id;
        f.advance(30_000);
        f.service.tick().await;
        f.advance(20_000);
        f.apps.lock().unwrap().as_mut().unwrap()[0].title = "other.ts".into();
        f.service.tick().await;
        assert_eq!(
            f.service.state.recording.as_ref().unwrap().session_id,
            session
        );
        assert_eq!(f.saved_usage().totals["r1"], 50_000);
        f.advance(10_000);
        f.set_apps(vec![]);
        f.service.tick().await;
        let saved = f.saved_usage();
        assert_eq!(saved.totals["r1"], 60_000);
        assert_eq!(saved.sessions[0].duration_ms, 60_000);
        assert_eq!(saved.sessions[0].ended_at, 61_000);
        validate_usage_data(&serde_json::to_value(&saved).unwrap()).unwrap();
        let mut restarted = Fixture::with_data(saved, ArchiveData::default());
        assert!(restarted.service.state.recording.is_none());
        restarted.automatic(true);
        restarted.service.tick().await;
        restarted.advance(15_000);
        restarted.service.tick().await;
        assert_eq!(restarted.service.state.usage_totals["r1"], 75_000);
        assert_eq!(restarted.service.state.usage_sessions.len(), 2);
    }

    #[tokio::test]
    async fn recording_toggle_and_process_restart_create_sessions_without_resetting_elapsed_timer()
    {
        let mut f = Fixture::new();
        f.automatic(true);
        f.service.tick().await;
        let since = f.service.state.since;
        f.advance(10_000);
        f.record(false).await;
        f.advance(60_000);
        f.service.tick().await;
        assert_eq!(f.service.state.usage_totals["r1"], 10_000);
        f.record(true).await;
        f.advance(20_000);
        f.service.tick().await;
        assert_eq!(f.service.state.usage_totals["r1"], 30_000);
        assert_eq!(f.service.state.usage_sessions.len(), 2);
        assert_eq!(f.service.state.since, since);
        assert_eq!(f.calls().len(), 1);
        f.advance(15_000);
        f.apps.lock().unwrap().as_mut().unwrap()[0].pid = 200;
        f.service.tick().await;
        assert_eq!(f.service.state.usage_sessions.len(), 3);
        f.advance(5_000);
        f.service.stop().await.unwrap();
        assert_eq!(f.service.state.usage_totals["r1"], 50_000);
        assert_eq!(f.service.state.usage_sessions[0].duration_ms, 5_000);
    }

    #[tokio::test]
    async fn ordered_rule_priority_preserves_rule_identity_even_with_shared_profile() {
        let mut f = Fixture::new();
        f.automatic(true);
        let mut second = f.service.config.rules[0].clone();
        second.id = "r2".into();
        second.name = "Notes".into();
        second.process = "notepad.exe".into();
        f.service.config.rules.push(second);
        f.apps.lock().unwrap().as_mut().unwrap().push(DetectedApp {
            pid: 101,
            process: "notepad.exe".into(),
            title: "Notes".into(),
            name: None,
        });
        f.service.tick().await;
        f.advance(30_000);
        let mut config = f.service.config.clone();
        config.rules.reverse();
        f.service.set_config(config).await.unwrap();
        f.service.tick().await;
        assert_eq!(f.service.state.active_rule_id.as_deref(), Some("r2"));
        f.advance(20_000);
        f.service.tick().await;
        assert_eq!(
            f.service.state.usage_totals,
            BTreeMap::from([("r1".into(), 30_000), ("r2".into(), 20_000)])
        );
        let mut config = f.service.config.clone();
        config.rules[0].enabled = false;
        f.service.set_config(config).await.unwrap();
        f.service.tick().await;
        f.advance(10_000);
        f.service.tick().await;
        assert_eq!(f.service.state.usage_totals["r1"], 40_000);
        assert_eq!(f.service.state.usage_sessions.len(), 3);
    }

    #[tokio::test]
    async fn failed_publishes_disconnected_time_and_shutdown_checkpoint_are_accounted_correctly() {
        let mut f = Fixture::new();
        f.automatic(true);
        f.rpc.lock().unwrap().fail = true;
        f.service.tick().await;
        assert!(f.service.state.usage_sessions.is_empty());
        f.advance(30_000);
        f.rpc.lock().unwrap().fail = false;
        f.service.tick().await;
        f.advance(20_000);
        f.service.on_disconnect(f.now.load(Ordering::SeqCst));
        assert!(f.service.state.recording.is_none());
        f.advance(3_600_000);
        f.service.tick().await;
        f.advance(5_000);
        f.service.dispose().await;
        assert_eq!(f.saved_usage().totals["r1"], 25_000);
        assert_eq!(f.saved_usage().sessions.len(), 2);
        assert!(f.service.state.recording.is_none());
    }

    #[tokio::test]
    async fn transport_timestamp_excludes_disconnected_time_before_listener_gets_the_mutex() {
        let mut f = Fixture::new();
        f.automatic(true);
        f.service.tick().await;
        f.advance(20_000);
        {
            let mut rpc = f.rpc.lock().unwrap();
            rpc.disconnected_at = Some(f.now.load(Ordering::SeqCst));
            rpc.connected = false;
        }
        // A long operation or queued timer can run before the outer listener
        // receives the service mutex. It still uses the transport's close time.
        f.advance(60_000);
        f.service.tick().await;
        assert_eq!(f.saved_usage().totals["r1"], 20_000);
        assert_eq!(f.service.state.usage_sessions.len(), 2);
        assert_eq!(f.service.state.usage_sessions[1].ended_at, 21_000);
        assert_eq!(f.service.state.usage_sessions[0].started_at, 81_000);
        f.advance(5_000);
        f.service.dispose().await;
        assert_eq!(f.saved_usage().totals["r1"], 25_000);
    }

    #[tokio::test]
    async fn failed_connect_never_starts_recording_and_publishes_connecting_before_error() {
        let mut f = Fixture::new();
        f.automatic(true);
        f.rpc.lock().unwrap().fail_connect = true;
        let changes = Arc::new(Mutex::new(Vec::<RuntimeState>::new()));
        let target = changes.clone();
        f.service
            .set_on_change(Arc::new(move |state| target.lock().unwrap().push(state)));
        f.service.tick().await;
        assert!(f.service.state.recording.is_none());
        assert!(f.saved_usage().sessions.is_empty());
        assert_eq!(f.service.state.connection, "disconnected");
        assert_eq!(
            f.service.state.error.as_deref(),
            Some("Connection rejected")
        );
        let changes = changes.lock().unwrap();
        assert!(changes.iter().any(|state| state.connection == "connecting"));
        assert!(!changes.iter().any(|state| state.connection == "connected"));
    }

    #[tokio::test]
    async fn failed_usage_writes_retry_without_interrupting_presence() {
        let mut f = Fixture::new();
        f.fail_usage.store(true, Ordering::SeqCst);
        f.automatic(true);
        f.service.tick().await;
        assert_eq!(f.service.state.connection, "connected");
        assert_eq!(
            f.service.state.logs[0].message,
            "Application time could not be saved. It will be retried."
        );
        f.advance(10_000);
        let mut config = f.service.config.clone();
        config.automatic = false;
        f.service.set_config(config).await.unwrap();
        f.fail_usage.store(false, Ordering::SeqCst);
        f.service.tick().await;
        assert_eq!(f.saved_usage().totals["r1"], 10_000);
    }

    #[tokio::test]
    async fn playback_duration_change_only_restarts_playback_timer() {
        let mut f = Fixture::new();
        f.automatic(true);
        f.service.config.profiles[0].r#type = 2;
        f.service.config.profiles[0].progress = true;
        f.service.tick().await;
        let session = f.service.state.recording.clone().unwrap().session_id;
        assert_eq!(
            f.calls().last().unwrap()["timestamps"],
            json!({"start":1,"end":181})
        );
        f.advance(30_000);
        f.apps.lock().unwrap().as_mut().unwrap()[0].title = "New track".into();
        f.service.tick().await;
        assert_eq!(f.service.state.since, Some(1_000));
        f.advance(10_000);
        let mut config = f.service.config.clone();
        config.profiles[0].duration = "4:00".into();
        f.service.set_config(config).await.unwrap();
        assert_eq!(f.service.state.since, Some(41_000));
        assert_eq!(
            f.calls().last().unwrap()["timestamps"],
            json!({"start":41,"end":281})
        );
        assert_eq!(
            f.service.state.recording.as_ref().unwrap().session_id,
            session
        );
        assert_eq!(f.service.state.usage_totals["r1"], 40_000);
        f.advance(15_000);
        f.service.tick().await;
        assert_eq!(f.service.state.usage_totals["r1"], 55_000);
    }

    #[tokio::test]
    async fn application_archive_keeps_the_total_and_updates_the_same_entry_after_each_session() {
        let mut f = Fixture::new();
        f.automatic(true);
        f.service.tick().await;
        f.advance(15_000);
        let session = f
            .service
            .state
            .recording
            .as_ref()
            .unwrap()
            .session_id
            .clone();
        f.service
            .archive_activity("application", "r1")
            .await
            .unwrap();
        let original = f.saved_archive().entries[0].clone();
        assert!(original.tracks_application_time());
        assert_eq!(original.entry["durationMs"], 15_000);
        assert_eq!(f.saved_usage().totals["r1"], 15_000);
        assert_eq!(
            f.service.state.recording.as_ref().unwrap().session_id,
            session
        );
        assert!(f
            .service
            .delete_activity("application", "r1")
            .await
            .unwrap_err()
            .contains("Pause"));
        f.advance(5_000);
        f.service.tick().await;
        assert_eq!(f.saved_usage().totals["r1"], 20_000);
        assert_eq!(
            f.saved_archive().entries[0],
            original,
            "checkpoints keep the archive at its last completed value"
        );
        f.service
            .archive_activity("application", "r1")
            .await
            .unwrap();
        assert_eq!(f.saved_archive().entries.len(), 1, "Archive is idempotent");
        f.advance(2_000);
        f.record(false).await;
        assert_eq!(f.service.state.usage_totals["r1"], 22_000);
        assert_eq!(f.service.state.usage_sessions.len(), 1);
        let archive = f.saved_archive();
        assert_eq!(archive.entries[0].entry["durationMs"], 22_000);
        assert_eq!(archive.entries[0].entry["name"], "Editor");
        assert_eq!(archive.entries[0].entry["id"], original.entry["id"]);
        assert_eq!(archive.entries[0].archived_at, original.archived_at);
        validate_archive_data(&serde_json::to_value(&archive).unwrap()).unwrap();
        f.record(true).await;
        f.advance(7_000);
        f.service.tick().await;
        assert_eq!(f.saved_archive().entries[0].entry["durationMs"], 22_000);
        f.record(false).await;
        assert_eq!(f.service.state.usage_totals["r1"], 29_000);
        assert_eq!(f.saved_archive().entries.len(), 1);
        assert_eq!(f.saved_archive().entries[0].entry["durationMs"], 29_000);
        assert_eq!(
            f.saved_usage().total_ids.unwrap()["r1"],
            archive.entries[0].entry["id"]
        );
        let restarted = Fixture::with_data(f.saved_usage(), f.saved_archive());
        assert_eq!(restarted.service.state.usage_totals["r1"], 29_000);
        assert_eq!(
            restarted.service.state.archive[0].entry["durationMs"],
            29_000
        );
        f.service
            .delete_activity("application", "r1")
            .await
            .unwrap();
        assert!(f.saved_usage().totals.is_empty());
        assert_eq!(f.saved_usage().sessions.len(), 2);
        assert_eq!(f.saved_archive().entries[0].entry["durationMs"], 29_000);
        assert!(!f.saved_archive().entries[0].tracks_application_time());
        f.record(true).await;
        f.advance(3_000);
        f.record(false).await;
        assert_eq!(f.saved_usage().totals["r1"], 3_000);
        assert_ne!(
            f.saved_usage().total_ids.unwrap()["r1"],
            original.entry["id"]
        );
        assert_eq!(
            f.saved_archive().entries[0].entry["durationMs"],
            29_000,
            "a deleted total's saved archive stays fixed"
        );
        f.service
            .archive_activity("application", "r1")
            .await
            .unwrap();
        assert_eq!(f.saved_archive().entries.len(), 2);
        assert!(f.saved_archive().entries[0].tracks_application_time());
        assert!(!f.saved_archive().entries[1].tracks_application_time());
    }

    #[tokio::test]
    async fn archive_creation_and_deletion_failures_preserve_totals_and_links() {
        let original = UsageData {
            totals: BTreeMap::from([("r1".into(), 12_000)]),
            ..Default::default()
        };
        let mut f = Fixture::with_data(original.clone(), ArchiveData::default());
        f.fail_usage.store(true, Ordering::SeqCst);
        f.fail_archive.store(true, Ordering::SeqCst);
        assert_eq!(
            f.service
                .delete_activity("application", "r1")
                .await
                .unwrap_err(),
            "Usage unavailable"
        );
        assert_eq!(f.service.state.usage_totals["r1"], 12_000);
        f.fail_usage.store(false, Ordering::SeqCst);
        assert_eq!(
            f.service
                .archive_activity("application", "r1")
                .await
                .unwrap_err(),
            "Archive unavailable"
        );
        assert_eq!(f.service.state.usage_totals["r1"], 12_000);
        f.fail_archive.store(false, Ordering::SeqCst);
        f.service
            .archive_activity("application", "r1")
            .await
            .unwrap();
        let archive = f.saved_archive();
        assert_eq!(archive.entries[0].entry["id"], "legacy:r1");
        assert_eq!(
            Fixture::with_data(original, archive.clone())
                .service
                .state
                .usage_totals["r1"],
            12_000
        );
        let id = archive.entries[0].entry["id"].as_str().unwrap();
        f.fail_archive.store(true, Ordering::SeqCst);
        assert_eq!(
            f.service
                .delete_archived_activity("application", id)
                .await
                .unwrap_err(),
            "Archive unavailable"
        );
        assert_eq!(f.saved_archive().entries.len(), 1);
        f.fail_archive.store(false, Ordering::SeqCst);
        f.service
            .delete_archived_activity("application", id)
            .await
            .unwrap();
        assert!(f.saved_archive().entries.is_empty());
        assert_eq!(f.service.state.usage_totals["r1"], 12_000);
        f.automatic(true);
        f.service.tick().await;
        f.advance(5_000);
        f.record(false).await;
        assert_eq!(f.saved_usage().totals["r1"], 17_000);
        assert!(
            f.saved_archive().entries.is_empty(),
            "deleting an archive stops its subscription"
        );
    }

    #[tokio::test]
    async fn linked_archive_write_failures_retry_and_recover_after_restart() {
        let mut f = Fixture::new();
        f.automatic(true);
        f.service.tick().await;
        f.service
            .archive_activity("application", "r1")
            .await
            .unwrap();
        f.fail_archive.store(true, Ordering::SeqCst);
        f.advance(15_000);
        f.record(false).await;
        assert_eq!(f.saved_usage().totals["r1"], 15_000);
        assert_eq!(f.saved_archive().entries[0].entry["durationMs"], 0);
        assert_eq!(f.service.state.archive[0].entry["durationMs"], 15_000);
        assert_eq!(f.service.flush_usage().unwrap_err(), "Archive unavailable");
        assert!(f.service.state.logs.iter().any(|log| log
            .message
            .contains("Archived application time could not be updated")));

        let mut restarted = Fixture::with_data(f.saved_usage(), f.saved_archive());
        assert_eq!(restarted.service.state.usage_totals["r1"], 15_000);
        assert_eq!(
            restarted.service.state.archive[0].entry["durationMs"],
            15_000
        );
        restarted.service.flush_usage().unwrap();
        assert_eq!(
            restarted.saved_archive().entries[0].entry["durationMs"],
            15_000
        );
        f.fail_archive.store(false, Ordering::SeqCst);
        f.service.tick().await;
        assert_eq!(f.saved_archive().entries[0].entry["durationMs"], 15_000);

        f.record(true).await;
        f.advance(5_000);
        f.fail_usage.store(true, Ordering::SeqCst);
        f.record(false).await;
        assert_eq!(
            f.saved_archive().entries[0].entry["durationMs"],
            15_000,
            "archive writes wait for the total's durable checkpoint"
        );
        f.service
            .log("info", "Save an event alongside the pending total update");
        let event_id = f.service.state.logs[0].id.clone();
        assert_eq!(
            f.service
                .archive_activity("event", &event_id)
                .await
                .unwrap_err(),
            "Usage unavailable"
        );
        assert_eq!(f.saved_archive().entries[0].entry["durationMs"], 15_000);
        f.fail_usage.store(false, Ordering::SeqCst);
        f.service.tick().await;
        assert_eq!(f.saved_usage().totals["r1"], 20_000);
        assert_eq!(f.saved_archive().entries[0].entry["durationMs"], 20_000);
        f.service
            .archive_activity("event", &event_id)
            .await
            .unwrap();
        assert_eq!(f.saved_archive().entries[1].entry["durationMs"], 20_000);
    }

    #[tokio::test]
    async fn linked_archives_finish_on_close_disconnect_manual_takeover_stop_and_shutdown() {
        for ending in [
            "close",
            "disconnect",
            "manual",
            "stop",
            "shutdown",
            "restart",
        ] {
            let mut f = Fixture::new();
            f.automatic(true);
            f.service.tick().await;
            f.service
                .archive_activity("application", "r1")
                .await
                .unwrap();
            f.advance(5_000);
            f.service.tick().await;
            assert_eq!(f.saved_archive().entries[0].entry["durationMs"], 0);
            f.advance(10_000);
            match ending {
                "close" => {
                    f.set_apps(vec![]);
                    f.service.tick().await;
                }
                "disconnect" => f.service.on_disconnect(f.now.load(Ordering::SeqCst)),
                "manual" => f.activate().await,
                "stop" => f.service.stop().await.unwrap(),
                "shutdown" => f.service.dispose().await,
                "restart" => {
                    f.set_apps(vec![DetectedApp {
                        pid: 200,
                        process: "Code.exe".into(),
                        title: "Restarted".into(),
                        ..Default::default()
                    }]);
                    f.service.tick().await;
                }
                _ => unreachable!(),
            }
            assert_eq!(
                f.saved_archive().entries[0].entry["durationMs"],
                15_000,
                "{ending}"
            );
            assert_eq!(f.saved_usage().totals["r1"], 15_000, "{ending}");
        }
    }

    #[tokio::test]
    async fn application_switch_updates_only_the_finished_app_and_session_archives_stay_fixed() {
        let mut f = Fixture::new();
        f.automatic(true);
        f.service.config.rules.push(AppRule {
            id: "r2".into(),
            name: "Notes".into(),
            process: "notepad.exe".into(),
            ..f.service.config.rules[0].clone()
        });
        f.service.tick().await;
        let first_session = f
            .service
            .state
            .recording
            .as_ref()
            .unwrap()
            .session_id
            .clone();
        f.service
            .archive_activity("application", "r1")
            .await
            .unwrap();
        f.advance(10_000);
        f.set_apps(vec![DetectedApp {
            pid: 101,
            process: "notepad.exe".into(),
            title: "Notes".into(),
            ..Default::default()
        }]);
        f.service.tick().await;
        assert_eq!(f.saved_archive().entries[0].entry["durationMs"], 10_000);
        f.service
            .archive_activity("session", &first_session)
            .await
            .unwrap();
        let saved_session = f.saved_archive().entries[0].clone();
        f.service
            .archive_activity("application", "r2")
            .await
            .unwrap();
        f.advance(6_000);
        f.service.tick().await;
        assert_eq!(f.saved_archive().entries[0].entry["durationMs"], 0);
        f.record(false).await;
        assert_eq!(f.saved_archive().entries[0].entry["durationMs"], 6_000);
        assert_eq!(f.saved_archive().entries[1], saved_session);
        assert_eq!(f.saved_archive().entries[2].entry["durationMs"], 10_000);
    }

    #[tokio::test]
    async fn deleted_totals_stay_deleted_if_the_archive_detach_write_is_interrupted() {
        let mut f = Fixture::with_data(
            UsageData {
                totals: BTreeMap::from([("r1".into(), 12_000)]),
                ..Default::default()
            },
            ArchiveData::default(),
        );
        f.service
            .archive_activity("application", "r1")
            .await
            .unwrap();
        f.fail_usage.store(true, Ordering::SeqCst);
        assert_eq!(
            f.service
                .delete_activity("application", "r1")
                .await
                .unwrap_err(),
            "Usage unavailable"
        );
        assert!(f.service.state.archive[0].tracks_application_time());
        f.fail_usage.store(false, Ordering::SeqCst);
        f.fail_archive.store(true, Ordering::SeqCst);
        f.service
            .delete_activity("application", "r1")
            .await
            .unwrap();
        assert!(f.saved_usage().totals.is_empty());
        assert!(
            f.saved_archive().entries[0].tracks_application_time(),
            "simulate a stale archive file"
        );
        let mut restarted = Fixture::with_data(f.saved_usage(), f.saved_archive());
        assert!(restarted.service.state.usage_totals.is_empty());
        assert!(!restarted.service.state.archive[0].tracks_application_time());
        assert_eq!(
            restarted.service.state.archive[0].entry["durationMs"],
            12_000
        );
        restarted.service.flush_usage().unwrap();
        assert!(!restarted.saved_archive().entries[0].tracks_application_time());
    }

    #[test]
    fn legacy_application_snapshots_remain_fixed_and_keep_interrupted_move_recovery() {
        let archive = validate_archive_data(&json!({"entries":[{"kind":"application","archivedAt":20000,"entry":{"id":"legacy:r1","ruleId":"r1","name":"Editor","process":"Code.exe","durationMs":10000}}]})).unwrap();
        let interrupted = Fixture::with_data(
            UsageData {
                totals: BTreeMap::from([("r1".into(), 10_000)]),
                ..Default::default()
            },
            archive.clone(),
        );
        assert!(interrupted.service.state.usage_totals.is_empty());
        let fresh = Fixture::with_data(
            UsageData {
                totals: BTreeMap::from([("r1".into(), 3_000)]),
                total_ids: Some(BTreeMap::from([("r1".into(), "new-total".into())])),
                ..Default::default()
            },
            archive.clone(),
        );
        assert_eq!(fresh.service.state.usage_totals["r1"], 3_000);
        assert_eq!(fresh.service.state.archive, archive.entries);
    }

    #[tokio::test]
    async fn archived_events_survive_eviction_and_delete_adds_no_log() {
        let mut f = Fixture::new();
        f.service.log("success", "Keep this event");
        let entry = f.service.state.logs[0].clone();
        f.service
            .archive_activity("event", &entry.id)
            .await
            .unwrap();
        assert!(f.service.state.logs.is_empty());
        assert_eq!(
            f.saved_archive().entries[0].entry,
            serde_json::to_value(entry).unwrap()
        );
        for index in 0..125 {
            f.service.log("info", &format!("Ordinary event {index}"));
        }
        assert_eq!(f.service.state.logs.len(), 100);
        let id = f.service.state.logs[0].id.clone();
        f.service.delete_activity("event", &id).await.unwrap();
        assert_eq!(f.service.state.logs.len(), 99);
        let restarted = Fixture::with_data(UsageData::default(), f.saved_archive());
        assert_eq!(restarted.service.state.archive.len(), 1);
        assert!(restarted.service.state.logs.is_empty());
    }

    #[tokio::test]
    async fn session_archive_keeps_total_and_recovery_removes_duplicate_history() {
        let mut f = Fixture::new();
        f.automatic(true);
        f.service.tick().await;
        let id = f
            .service
            .state
            .recording
            .as_ref()
            .unwrap()
            .session_id
            .clone();
        assert!(f
            .service
            .archive_activity("session", &id)
            .await
            .unwrap_err()
            .contains("Finish"));
        assert!(f
            .service
            .delete_activity("session", &id)
            .await
            .unwrap_err()
            .contains("Finish"));
        f.advance(12_000);
        f.service.stop().await.unwrap();
        let before = f.saved_usage();
        f.service.archive_activity("session", &id).await.unwrap();
        assert!(f.saved_usage().sessions.is_empty());
        assert_eq!(f.saved_usage().totals["r1"], 12_000);
        let mut restarted = Fixture::with_data(before, f.saved_archive());
        assert!(restarted.service.state.usage_sessions.is_empty());
        restarted.service.tick().await;
        assert!(restarted.saved_usage().sessions.is_empty());
        assert_eq!(restarted.saved_usage().totals["r1"], 12_000);
    }

    fn completed_session() -> UsageData {
        UsageData {
            totals: BTreeMap::from([("r1".into(), 10_000)]),
            sessions: vec![UsageSession {
                id: "session".into(),
                rule_id: "r1".into(),
                name: "Editor".into(),
                process: "Code.exe".into(),
                started_at: 1_000,
                ended_at: 11_000,
                duration_ms: 10_000,
                extra: Default::default(),
            }],
            total_ids: None,
        }
    }

    #[tokio::test]
    async fn deleting_session_rolls_back_failed_write_without_changing_total() {
        let mut f = Fixture::with_data(completed_session(), ArchiveData::default());
        f.fail_usage.store(true, Ordering::SeqCst);
        assert_eq!(
            f.service
                .delete_activity("session", "session")
                .await
                .unwrap_err(),
            "Usage unavailable"
        );
        assert_eq!(f.service.state.usage_sessions.len(), 1);
        assert_eq!(f.service.state.usage_totals["r1"], 10_000);
        f.fail_usage.store(false, Ordering::SeqCst);
        f.service
            .delete_activity("session", "session")
            .await
            .unwrap();
        assert!(f.saved_usage().sessions.is_empty());
        assert_eq!(f.saved_usage().totals["r1"], 10_000);
    }

    #[tokio::test]
    async fn archived_session_remains_saved_until_pending_history_cleanup_persists() {
        let mut f = Fixture::with_data(completed_session(), ArchiveData::default());
        f.fail_usage.store(true, Ordering::SeqCst);
        f.service
            .archive_activity("session", "session")
            .await
            .unwrap();
        assert_eq!(f.saved_archive().entries.len(), 1);
        assert!(f.service.state.usage_sessions.is_empty());
        assert_eq!(
            f.service
                .delete_archived_activity("session", "session")
                .await
                .unwrap_err(),
            "Usage unavailable"
        );
        assert_eq!(f.saved_archive().entries.len(), 1);
        f.fail_usage.store(false, Ordering::SeqCst);
        f.service
            .delete_archived_activity("session", "session")
            .await
            .unwrap();
        assert!(f.saved_usage().sessions.is_empty());
        assert!(f.saved_archive().entries.is_empty());
        assert_eq!(f.saved_usage().totals["r1"], 10_000);
    }

    #[tokio::test]
    async fn archive_failures_preserve_entries_and_invalid_references_are_rejected() {
        let mut f = Fixture::new();
        f.service.log("info", "Keep until saved");
        let id = f.service.state.logs[0].id.clone();
        f.fail_archive.store(true, Ordering::SeqCst);
        assert_eq!(
            f.service.archive_activity("event", &id).await.unwrap_err(),
            "Archive unavailable"
        );
        assert_eq!(f.service.state.logs.len(), 1);
        assert!(f.service.state.archive.is_empty());
        f.fail_archive.store(false, Ordering::SeqCst);
        f.service.archive_activity("event", &id).await.unwrap();
        f.fail_archive.store(true, Ordering::SeqCst);
        assert_eq!(
            f.service
                .delete_archived_activity("event", &id)
                .await
                .unwrap_err(),
            "Archive unavailable"
        );
        assert_eq!(f.service.state.archive.len(), 1);
        f.fail_archive.store(false, Ordering::SeqCst);
        f.service
            .delete_archived_activity("event", &id)
            .await
            .unwrap();
        assert!(f.saved_archive().entries.is_empty());
        assert!(f
            .service
            .delete_activity("event", &id)
            .await
            .unwrap_err()
            .contains("no longer exists"));
        assert_eq!(
            f.service.delete_activity("invalid", &id).await.unwrap_err(),
            "Invalid activity entry."
        );
        assert_eq!(
            f.service.archive_activity("event", "").await.unwrap_err(),
            "Invalid activity entry."
        );
    }
}
