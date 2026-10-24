//! Native editor state and projections. No windowing or rendering framework is required.
//!
//! Candidate operations leave the editor untouched until persistence succeeds. The host
//! commits the configuration returned by `Backend::save_config` with [`Editor::accept`].
use presence_core::{engine, model::*};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Editor {
    pub config: Config,
    pub draft: Profile,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preview {
    pub app_name: String,
    pub details: String,
    pub state: String,
    pub large_text: String,
    pub small_text: String,
    pub activity_label: String,
    pub elapsed_text: String,
    pub playback_position: String,
    pub playback_duration: String,
    pub playback_fraction: f32,
    pub has_playback: bool,
    pub has_tokens: bool,
    pub effective_application_id: String,
    pub validation_error: String,
    pub publishing_error: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivityRow {
    /// Backend entry kind: `application`, `event`, or `session`.
    pub kind: String,
    /// Rule ID for a live total, original entry ID for all other rows.
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub time: String,
    pub detail: String,
    pub level: String,
    pub duration_ms: i64,
    pub timestamp: i64,
    pub archived: bool,
    /// Recording rows cannot be archived or deleted until recording pauses.
    pub locked: bool,
}

impl Editor {
    /// Backend configuration is validated on load and always has a profile.
    pub fn new(config: Config) -> Self {
        let draft = config.profiles.first().cloned().unwrap_or_default();
        Self { config, draft }
    }

    pub fn selected_id(&self) -> &str {
        &self.draft.id
    }

    pub fn dirty(&self) -> bool {
        self.config.profiles.iter().find(|p| p.id == self.draft.id) != Some(&self.draft)
    }

    /// The host may offer save/discard/cancel when this returns an unsaved-edits error.
    pub fn select(&mut self, id: &str) -> Result<(), String> {
        if id == self.draft.id {
            return Ok(());
        }
        let profile = self
            .config
            .profiles
            .iter()
            .find(|p| p.id == id)
            .ok_or("This presence no longer exists.")?;
        if self.dirty() {
            return Err(format!("You have unsaved edits to “{}”.", self.draft.name));
        }
        self.draft = profile.clone();
        Ok(())
    }

    pub fn discard(&mut self) {
        if let Some(profile) = self.config.profiles.iter().find(|p| p.id == self.draft.id) {
            self.draft = profile.clone();
        }
    }

    /// Explicit selection reloads the draft. An unrelated config update preserves
    /// unsaved edits when the saved version of that profile has not changed.
    pub fn accept(&mut self, config: Config, selection: Option<&str>) {
        let old_saved = self.config.profiles.iter().find(|p| p.id == self.draft.id);
        let id = selection.unwrap_or(&self.draft.id);
        let next = config
            .profiles
            .iter()
            .find(|p| p.id == id)
            .or_else(|| config.profiles.first());
        if let Some(next) = next {
            let preserve = selection.is_none() && self.dirty() && old_saved == Some(next);
            if !preserve {
                self.draft = next.clone();
            }
        }
        self.config = config;
    }

    /// Field names accept the JSON camelCase, Rust snake_case, or Slint kebab-case.
    /// Buttons use one-based `button1Label`/`button1Url` and
    /// `button2Label`/`button2Url`. `button0*` aliases the first button.
    pub fn set_field(&mut self, field: &str, value: &str) -> Result<(), String> {
        let key = field_key(field);
        match key.as_str() {
            "name" | "profilename" => self.draft.name = value.into(),
            "applicationid" => self.draft.application_id = value.into(),
            "appname" | "displayname" => self.draft.app_name = value.into(),
            "type" | "activitytype" => self.draft.r#type = parse_activity_type(value)?,
            "details" => self.draft.details = value.into(),
            "state" => self.draft.state = value.into(),
            "largeimage" => self.draft.large_image = value.into(),
            "largetext" => self.draft.large_text = value.into(),
            "smallimage" => self.draft.small_image = value.into(),
            "smalltext" => self.draft.small_text = value.into(),
            "elapsed" => self.draft.elapsed = parse_bool(value)?,
            "progress" => self.draft.progress = parse_bool(value)?,
            "duration" => self.draft.duration = value.into(),
            "theme" => {
                if !matches!(value, "violet" | "mint" | "peach" | "blue") {
                    return Err("Choose a supported preview theme.".into());
                }
                self.draft.theme = value.into();
            }
            "button0label" | "button0url" | "button1label" | "button1url" | "button2label"
            | "button2url" => {
                let index = usize::from(key.starts_with("button2"));
                while self.draft.buttons.len() <= index {
                    self.draft.buttons.push(Button::default());
                }
                if key.ends_with("label") {
                    self.draft.buttons[index].label = value.into();
                } else {
                    self.draft.buttons[index].url = value.into();
                }
            }
            _ => return Err(format!("Unknown profile field: {field}")),
        }
        Ok(())
    }

    pub fn add_button(&mut self) -> Result<(), String> {
        if self.draft.buttons.len() >= 2 {
            return Err("Discord supports up to two buttons.".into());
        }
        self.draft.buttons.push(Button::default());
        Ok(())
    }

    pub fn remove_button(&mut self, index: usize) -> Result<(), String> {
        if index >= self.draft.buttons.len() {
            return Err("This button no longer exists.".into());
        }
        self.draft.buttons.remove(index);
        Ok(())
    }

    pub fn candidate_save(&self) -> Result<Config, String> {
        engine::validate_profile(&self.draft, false, &self.config.application_id)?;
        let mut next = self.config.clone();
        let saved = next
            .profiles
            .iter_mut()
            .find(|p| p.id == self.draft.id)
            .ok_or("This presence no longer exists.")?;
        *saved = self.draft.clone();
        validated(next)
    }

    /// Going live saves a pending global ID when this presence inherits it.
    /// A profile override remains independent of unrelated Settings edits.
    pub fn candidate_activate(&self, global_id_draft: &str) -> Result<Config, String> {
        let mut next = self.candidate_save()?;
        if self.draft.application_id.is_empty() {
            next.application_id = global_id_draft.trim().into();
        }
        let next = validated(next)?;
        engine::validate_profile(&self.draft, true, &next.application_id)?;
        Ok(next)
    }

    /// New/duplicate also saves the current draft, matching the inherited studio.
    pub fn candidate_new(&self) -> Result<(Config, String), String> {
        self.candidate_add(false)
    }

    pub fn candidate_duplicate(&self) -> Result<(Config, String), String> {
        self.candidate_add(true)
    }

    fn candidate_add(&self, duplicate: bool) -> Result<(Config, String), String> {
        if self.config.profiles.len() >= 100 {
            return Err("A workspace supports up to 100 presences.".into());
        }
        let mut next = self.candidate_save()?;
        let profile = if duplicate {
            let mut copy = self.draft.clone();
            copy.id = uuid::Uuid::new_v4().to_string();
            copy.name = format!("{} copy", truncate_utf16(&copy.name, 55));
            copy
        } else {
            create_profile("New presence")
        };
        let id = profile.id.clone();
        next.profiles.push(profile);
        Ok((validated(next)?, id))
    }

    /// Deleting a presence also removes rules referring to it. History remains.
    pub fn candidate_delete(&self) -> Result<Config, String> {
        if self.config.profiles.len() <= 1 {
            return Err("Keep at least one presence in your workspace.".into());
        }
        let mut next = self.config.clone();
        if !next.profiles.iter().any(|p| p.id == self.draft.id) {
            return Err("This presence no longer exists.".into());
        }
        next.profiles.retain(|p| p.id != self.draft.id);
        next.rules.retain(|r| r.profile_id != self.draft.id);
        validated(next)
    }

    pub fn new_rule(&self) -> AppRule {
        AppRule {
            id: uuid::Uuid::new_v4().to_string(),
            profile_id: self.draft.id.clone(),
            enabled: true,
            ..AppRule::default()
        }
    }

    pub fn candidate_rule_upsert(&self, mut rule: AppRule) -> Result<Config, String> {
        let mut next = self.config.clone();
        if rule.id.is_empty() {
            rule.id = uuid::Uuid::new_v4().to_string();
        }
        if let Some(existing) = next.rules.iter_mut().find(|r| r.id == rule.id) {
            *existing = rule;
        } else {
            if next.rules.len() >= 100 {
                return Err("A workspace supports up to 100 application rules.".into());
            }
            next.rules.push(rule);
        }
        validated(next)
    }

    pub fn candidate_rule_delete(&self, id: &str) -> Result<Config, String> {
        let mut next = self.config.clone();
        let before = next.rules.len();
        next.rules.retain(|r| r.id != id);
        if before == next.rules.len() {
            return Err("This application rule no longer exists.".into());
        }
        validated(next)
    }

    pub fn candidate_rule_move(&self, id: &str, delta: i32) -> Result<Config, String> {
        let mut next = self.config.clone();
        let index = next
            .rules
            .iter()
            .position(|r| r.id == id)
            .ok_or("This application rule no longer exists.")?;
        let target = (index as i64 + i64::from(delta))
            .clamp(0, next.rules.len().saturating_sub(1) as i64) as usize;
        let rule = next.rules.remove(index);
        next.rules.insert(target, rule);
        validated(next)
    }

    pub fn candidate_rule_toggle(&self, id: &str) -> Result<Config, String> {
        let mut next = self.config.clone();
        let rule = next
            .rules
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or("This application rule no longer exists.")?;
        rule.enabled = !rule.enabled;
        validated(next)
    }

    pub fn candidate_rule_drop(
        &self,
        id: &str,
        target_id: &str,
        after: bool,
    ) -> Result<Config, String> {
        let mut next = self.config.clone();
        let index = next
            .rules
            .iter()
            .position(|rule| rule.id == id)
            .ok_or("This application rule no longer exists.")?;
        if id == target_id {
            return Ok(next);
        }
        let rule = next.rules.remove(index);
        let target = next
            .rules
            .iter()
            .position(|rule| rule.id == target_id)
            .ok_or("The target application rule no longer exists.")?;
        next.rules.insert(target + usize::from(after), rule);
        validated(next)
    }

    pub fn preview(&self, runtime: &RuntimeState, now_ms: i64) -> Preview {
        let example = Metadata {
            app: "Visual Studio Code".into(),
            title: "Presence Control — Visual Studio Code".into(),
            process: "Code.exe".into(),
        };
        let selected_is_live = runtime.active_profile_id.as_deref() == Some(&self.draft.id);
        let metadata = if selected_is_live {
            runtime.metadata.as_ref().unwrap_or(&example)
        } else {
            &example
        };
        let resolve = |text: &str| engine::resolve_template(text, Some(metadata));
        let elapsed_seconds = if selected_is_live {
            runtime
                .since
                .map(|since| now_ms.saturating_sub(since).max(0) / 1000)
                .unwrap_or(0)
        } else {
            0
        };
        let duration = engine::playback_duration_seconds(&self.draft);
        let position = duration
            .map(|duration| elapsed_seconds.min(duration))
            .unwrap_or(0);
        let app_name = resolve(&self.draft.app_name);
        Preview {
            app_name: if app_name.is_empty() {
                "Your application".into()
            } else {
                app_name
            },
            details: resolve(&self.draft.details),
            state: resolve(&self.draft.state),
            large_text: resolve(&self.draft.large_text),
            small_text: resolve(&self.draft.small_text),
            activity_label: activity_label(self.draft.r#type).into(),
            elapsed_text: if elapsed_seconds >= 3600 {
                format!(
                    "{:02}:{:02}:{:02} elapsed",
                    elapsed_seconds / 3600,
                    (elapsed_seconds / 60) % 60,
                    elapsed_seconds % 60
                )
            } else {
                format!(
                    "{:02}:{:02} elapsed",
                    elapsed_seconds / 60,
                    elapsed_seconds % 60
                )
            },
            playback_position: format_playback_time(position),
            playback_duration: format_playback_time(duration.unwrap_or(0)),
            playback_fraction: duration.map(|d| position as f32 / d as f32).unwrap_or(0.0),
            has_playback: duration.is_some(),
            has_tokens: [
                &self.draft.app_name,
                &self.draft.details,
                &self.draft.state,
                &self.draft.large_text,
                &self.draft.small_text,
            ]
            .iter()
            .any(|value| value.contains("{{")),
            effective_application_id: engine::application_id_for(
                &self.draft,
                &self.config.application_id,
            )
            .into(),
            validation_error: engine::validate_profile(
                &self.draft,
                false,
                &self.config.application_id,
            )
            .err()
            .unwrap_or_default(),
            publishing_error: engine::validate_profile(
                &self.draft,
                true,
                &self.config.application_id,
            )
            .err()
            .unwrap_or_default(),
        }
    }
}

pub fn set_rule_field(rule: &mut AppRule, field: &str, value: &str) -> Result<(), String> {
    match field_key(field).as_str() {
        "name" => rule.name = value.into(),
        "process" => rule.process = value.into(),
        "titlecontains" => rule.title_contains = value.into(),
        "profileid" => rule.profile_id = value.into(),
        "enabled" => rule.enabled = parse_bool(value)?,
        _ => return Err(format!("Unknown application rule field: {field}")),
    }
    Ok(())
}

pub fn validated(config: Config) -> Result<Config, String> {
    let value = serde_json::to_value(config).map_err(|e| e.to_string())?;
    engine::validate_config(&value)
}

pub fn activity_label(activity_type: u8) -> &'static str {
    match activity_type {
        2 => "Listening to",
        3 => "Watching",
        5 => "Competing in",
        _ => "Playing",
    }
}

fn field_key(field: &str) -> String {
    field
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | '.'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" | "1" | "on" => Ok(true),
        "false" | "0" | "off" => Ok(false),
        _ => Err("Choose an enabled or disabled value.".into()),
    }
}

fn parse_activity_type(value: &str) -> Result<u8, String> {
    match value {
        "0" | "Playing" => Ok(0),
        "2" | "Listening to" => Ok(2),
        "3" | "Watching" => Ok(3),
        "5" | "Competing in" => Ok(5),
        _ => Err("Choose a supported activity type.".into()),
    }
}

fn truncate_utf16(value: &str, limit: usize) -> String {
    let mut count = 0;
    value
        .chars()
        .take_while(|c| {
            count += c.len_utf16();
            count <= limit
        })
        .collect()
}

pub fn format_duration(milliseconds: i64) -> String {
    let seconds = milliseconds.max(0) / 1000;
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

pub fn format_playback_time(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds >= 3600 {
        format_duration(seconds.saturating_mul(1000))
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn format_local_time(timestamp: i64, pattern: &str) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format(pattern)
                .to_string()
        })
        .unwrap_or_else(|| "Unknown time".into())
}

/// Absolute wall-clock times, matching the inherited studio's local time display.
pub fn format_event_time(timestamp: i64) -> String {
    format_local_time(timestamp, "%I:%M:%S %p")
}

pub fn format_session_time(timestamp: i64) -> String {
    format_local_time(timestamp, "%b %-d, %Y, %-I:%M %p")
}

/// Groups application totals, events, and sessions as in the inherited activity
/// page. Each group preserves the backend's order. Search never changes IDs used
/// for archive/delete actions, and archived application totals use their archive
/// entry IDs rather than their rule IDs.
pub fn activity_rows(
    config: &Config,
    runtime: &RuntimeState,
    filter: &str,
    query: &str,
    archived: bool,
    now_ms: i64,
) -> Vec<ActivityRow> {
    let mut rows = if archived {
        runtime
            .archive
            .iter()
            .filter_map(|item| {
                let mut row = row_from_json(&item.kind, &item.entry)?;
                row.archived = true;
                row.subtitle = format!(
                    "{} · Archived {}",
                    row.subtitle,
                    format_session_time(item.archived_at)
                );
                row.timestamp = item.archived_at;
                Some(row)
            })
            .collect::<Vec<_>>()
    } else {
        let mut rows = total_rows(config, runtime, now_ms);
        rows.extend(runtime.logs.iter().map(|log| ActivityRow {
            kind: "event".into(),
            id: log.id.clone(),
            title: log.message.clone(),
            subtitle: log.level.clone(),
            time: format_event_time(log.at),
            detail: format!(
                "{}\n{} · {}",
                log.message,
                log.level,
                format_local_time(log.at, "%b %-d, %Y, %-I:%M:%S %p")
            ),
            level: log.level.clone(),
            timestamp: log.at,
            ..ActivityRow::default()
        }));
        rows.extend(runtime.usage_sessions.iter().map(|session| {
            let active = runtime
                .recording
                .as_ref()
                .filter(|r| r.session_id == session.id);
            let uncommitted = active
                .map(|r| now_ms.saturating_sub(r.since).max(0))
                .unwrap_or(0);
            let duration = session.duration_ms.saturating_add(uncommitted);
            ActivityRow {
                kind: "session".into(),
                id: session.id.clone(),
                title: session.name.clone(),
                subtitle: if active.is_some() {
                    format!("{} · Recording now", session.process)
                } else {
                    session.process.clone()
                },
                time: format_duration(duration),
                detail: if active.is_some() {
                    format!(
                        "{} · Started {} · Recording now. Pause recording to finish this session.",
                        session.process,
                        format_session_time(session.started_at)
                    )
                } else {
                    format!(
                        "{}\n{} – {}",
                        session.process,
                        format_session_time(session.started_at),
                        format_session_time(session.ended_at)
                    )
                },
                duration_ms: duration,
                timestamp: session.started_at,
                locked: active.is_some(),
                ..ActivityRow::default()
            }
        }));
        rows
    };
    let query = query.to_lowercase();
    let filter = match filter.to_lowercase().as_str() {
        "event" | "events" | "this session" => "event",
        "session" | "sessions" | "previous sessions" => "session",
        "application" | "applications" | "application time" => "application",
        _ => "all",
    };
    rows.retain(|row| {
        (filter == "all" || row.kind == filter)
            && (query.is_empty()
                || [&row.title, &row.subtitle, &row.detail]
                    .iter()
                    .any(|value| value.to_lowercase().contains(&query)))
    });
    rows
}

fn total_rows(config: &Config, runtime: &RuntimeState, now_ms: i64) -> Vec<ActivityRow> {
    let mut names = BTreeMap::<String, (String, String)>::new();
    for entry in runtime
        .archive
        .iter()
        .filter(|entry| entry.kind == "session")
    {
        if let Ok(session) = serde_json::from_value::<UsageSession>(entry.entry.clone()) {
            names.insert(session.rule_id, (session.name, session.process));
        }
    }
    for session in runtime.usage_sessions.iter().rev() {
        names.insert(
            session.rule_id.clone(),
            (session.name.clone(), session.process.clone()),
        );
    }
    for rule in &config.rules {
        names.insert(
            rule.id.clone(),
            (
                if rule.name.is_empty() {
                    rule.process.clone()
                } else {
                    rule.name.clone()
                },
                rule.process.clone(),
            ),
        );
    }
    let mut totals = runtime.usage_totals.clone();
    if let Some(recording) = &runtime.recording {
        let total = totals.entry(recording.rule_id.clone()).or_default();
        *total = total.saturating_add(now_ms.saturating_sub(recording.since).max(0));
    }
    totals.into_iter().map(|(id, duration_ms)| {
        let (name, process) = names.remove(&id).unwrap_or_else(|| ("Removed application".into(), String::new()));
        let locked = runtime.recording.as_ref().is_some_and(|recording| recording.rule_id == id);
        ActivityRow {
            kind: "application".into(),
            id,
            title: name,
            subtitle: if locked { "Recording now".into() } else { "Total recorded time".into() },
            time: format_duration(duration_ms),
            detail: if process.is_empty() { "Cumulative application time. Session history is kept when this total is reset.".into() }
                else { format!("{process} · Cumulative application time. Session history is kept when this total is reset.") },
            duration_ms,
            locked,
            ..ActivityRow::default()
        }
    }).collect()
}

fn row_from_json(kind: &str, entry: &serde_json::Value) -> Option<ActivityRow> {
    let text = |key| {
        entry
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let number = |key| {
        entry
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
    };
    let mut row = ActivityRow {
        kind: kind.into(),
        id: text("id"),
        ..ActivityRow::default()
    };
    if row.id.is_empty() {
        return None;
    }
    match kind {
        "event" => {
            row.title = text("message");
            row.level = text("level");
            row.subtitle = row.level.clone();
            row.timestamp = number("at");
            row.time = format_event_time(row.timestamp);
            row.detail = format!(
                "{}\n{} · {}",
                row.title,
                row.level,
                format_local_time(row.timestamp, "%b %-d, %Y, %-I:%M:%S %p")
            );
        }
        "session" | "application" => {
            row.title = text("name");
            row.duration_ms = number("durationMs");
            row.time = format_duration(row.duration_ms);
            row.timestamp = number("startedAt");
            row.subtitle = if kind == "application" {
                "Archived application time".into()
            } else {
                text("process")
            };
            row.detail = if kind == "application" {
                format!("{} · Cumulative time saved when archived. Future recordings start a new total.", text("process"))
            } else {
                format!(
                    "{}\n{} – {}",
                    text("process"),
                    format_session_time(number("startedAt")),
                    format_session_time(number("endedAt"))
                )
            };
        }
        _ => return None,
    }
    Some(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn editor() -> Editor {
        Editor::new(default_config())
    }

    #[test]
    fn drafts_are_explicit_and_failed_validation_leaves_saved_data_untouched() {
        let mut editor = editor();
        let original = editor.config.clone();
        editor.set_field("name", "").unwrap();
        assert!(editor.dirty());
        assert!(editor.candidate_save().is_err());
        assert_eq!(editor.config, original);
        assert!(editor.select("deep-work").unwrap_err().contains("unsaved"));
        editor.discard();
        assert!(!editor.dirty());
        editor.select("deep-work").unwrap();
        assert_eq!(editor.draft.name, "Deep work");
    }

    #[test]
    fn unrelated_config_save_preserves_draft_but_profile_save_commits_it() {
        let mut editor = editor();
        editor.set_field("details", "Still editing").unwrap();
        let mut config = editor.config.clone();
        config.record_time = true;
        editor.accept(config, None);
        assert!(editor.config.record_time);
        assert_eq!(editor.draft.details, "Still editing");
        assert!(editor.dirty());
        let candidate = editor.candidate_save().unwrap();
        editor.accept(candidate, None);
        assert!(!editor.dirty());
        assert_eq!(editor.draft.details, "Still editing");
    }

    #[test]
    fn activation_saves_pending_global_id_and_profile_edits_together() {
        let mut editor = editor();
        editor.set_field("details", "Ready to publish").unwrap();
        let original = editor.config.clone();
        let id = "1562501049397411983";
        let candidate = editor.candidate_activate(&format!("  {id}  ")).unwrap();
        assert_eq!(candidate.application_id, id);
        assert_eq!(candidate.profiles[0].details, "Ready to publish");
        assert!(candidate.profiles[0].application_id.is_empty());
        assert_eq!(editor.config, original, "wait for successful persistence");
        editor.accept(candidate, Some("creative-flow"));
        assert!(!editor.dirty());
        assert_eq!(
            editor
                .preview(&initial_runtime(), 0)
                .effective_application_id,
            id
        );
        assert!(editor.candidate_activate(id).is_ok());

        let replacement = "987654321098765432";
        let candidate = editor.candidate_activate(replacement).unwrap();
        assert_eq!(candidate.application_id, replacement);
        assert_eq!(editor.config.application_id, id);
    }

    #[test]
    fn invalid_global_edits_do_not_mutate_config_or_block_profile_overrides() {
        let mut editor = editor();
        editor.config.application_id = "1562501049397411983".into();
        let original = editor.config.clone();
        for id in ["", "invalid", "123"] {
            assert!(editor.candidate_activate(id).is_err());
            assert_eq!(editor.config, original);
        }
        editor
            .set_field("applicationId", "987654321098765432")
            .unwrap();
        for id in ["", "invalid", "123", "876543210987654321"] {
            let candidate = editor.candidate_activate(id).unwrap();
            assert_eq!(candidate.application_id, original.application_id);
            assert_eq!(candidate.profiles[0].application_id, "987654321098765432");
        }
        assert_eq!(editor.config, original);
    }

    #[test]
    fn new_and_duplicate_save_current_edits_without_early_commit() {
        let mut editor = editor();
        editor.draft.name = "🎧".repeat(30);
        let (candidate, id) = editor.candidate_duplicate().unwrap();
        assert_eq!(editor.config.profiles.len(), 3);
        assert_eq!(candidate.profiles.len(), 4);
        let copy = candidate.profiles.iter().find(|p| p.id == id).unwrap();
        assert!(copy.name.ends_with(" copy"));
        assert!(copy.name.encode_utf16().count() <= 60);
        editor.accept(candidate, Some(&id));
        assert!(!editor.dirty());
        let (candidate, id) = editor.candidate_new().unwrap();
        assert_eq!(
            candidate.profiles.iter().find(|p| p.id == id).unwrap().name,
            "New presence"
        );
    }

    #[test]
    fn removing_profile_cleans_rules_and_cannot_remove_last_profile() {
        let mut editor = editor();
        let mut rule = editor.new_rule();
        rule.process = "Code.exe".into();
        editor.config.rules.push(rule);
        let next = editor.candidate_delete().unwrap();
        assert_eq!(next.profiles.len(), 2);
        assert!(next.rules.is_empty());
        editor.accept(next, None);
        assert_eq!(editor.selected_id(), "deep-work");
        let next = editor.candidate_delete().unwrap();
        editor.accept(next, None);
        assert!(editor.candidate_delete().is_err());
    }

    #[test]
    fn unknown_fields_survive_edits_duplicates_and_rule_updates() {
        let mut editor = editor();
        editor
            .config
            .extra
            .insert("futureWorkspace".into(), json!(true));
        editor
            .draft
            .extra
            .insert("futureProfile".into(), json!({"value": 7}));
        editor.draft.buttons.push(Button {
            label: "Open".into(),
            url: "https://example.com".into(),
            extra: serde_json::Map::from_iter([("futureButton".into(), json!(1))]),
        });
        editor.set_field("button0-label", "Visit").unwrap();
        let (next, id) = editor.candidate_duplicate().unwrap();
        let copy = next.profiles.iter().find(|p| p.id == id).unwrap();
        assert_eq!(copy.extra["futureProfile"]["value"], 7);
        assert_eq!(copy.buttons[0].extra["futureButton"], 1);
        assert_eq!(next.extra["futureWorkspace"], true);
    }

    #[test]
    fn rule_order_is_bounded_and_invalid_updates_do_not_change_config() {
        let mut editor = editor();
        for process in ["Code.exe", "firefox.exe", "slack.exe"] {
            let mut rule = editor.new_rule();
            rule.process = process.into();
            editor.config.rules.push(rule);
        }
        let id = editor.config.rules[0].id.clone();
        let next = editor.candidate_rule_move(&id, 2).unwrap();
        assert_eq!(next.rules[2].id, id);
        assert_eq!(editor.config.rules[0].id, id);
        assert_eq!(editor.candidate_rule_move(&id, -10).unwrap(), editor.config);
        let mut rule = editor.config.rules[0].clone();
        rule.profile_id = "missing-profile".into();
        assert!(editor.candidate_rule_upsert(rule).is_err());
        assert!(!editor.candidate_rule_toggle(&id).unwrap().rules[0].enabled);
        assert_eq!(editor.candidate_rule_delete(&id).unwrap().rules.len(), 2);
    }

    #[test]
    fn preview_resolves_example_and_live_templates_with_clamped_playback() {
        let mut editor = editor();
        editor.draft.app_name = "{{app}}".into();
        editor.draft.details = "{{title}}".into();
        editor.draft.state = "{{process}}".into();
        editor.draft.r#type = 2;
        editor.draft.progress = true;
        editor.draft.duration = "3:00".into();
        let mut runtime = initial_runtime();
        let preview = editor.preview(&runtime, 500_000);
        assert_eq!(preview.app_name, "Visual Studio Code");
        assert!(preview.has_tokens && preview.has_playback);
        assert_eq!(preview.playback_fraction, 0.0);
        assert_eq!(preview.elapsed_text, "00:00 elapsed");
        assert!(preview.publishing_error.contains("application ID"));
        runtime.active_profile_id = Some(editor.draft.id.clone());
        runtime.since = Some(1_000);
        runtime.metadata = Some(Metadata {
            app: "Real app".into(),
            title: "Real title".into(),
            process: "real.exe".into(),
        });
        let preview = editor.preview(&runtime, 500_000);
        assert_eq!(preview.app_name, "Real app");
        assert_eq!(preview.details, "Real title");
        assert_eq!(preview.state, "real.exe");
        assert_eq!(preview.playback_position, "3:00");
        assert_eq!(preview.playback_fraction, 1.0);
    }

    #[test]
    fn rule_drops_preserve_identity_and_place_before_or_after_the_target() {
        let mut editor = editor();
        for process in ["first.exe", "second.exe", "third.exe", "fourth.exe"] {
            let mut rule = editor.new_rule();
            rule.process = process.into();
            rule.extra.insert("custom".into(), json!(process));
            editor.config.rules.push(rule);
        }
        let original = editor.config.clone();
        let ids: Vec<_> = original.rules.iter().map(|rule| rule.id.as_str()).collect();
        let down = editor.candidate_rule_drop(ids[0], ids[3], true).unwrap();
        assert_eq!(
            down.rules,
            vec![
                original.rules[1].clone(),
                original.rules[2].clone(),
                original.rules[3].clone(),
                original.rules[0].clone()
            ]
        );
        let up = editor.candidate_rule_drop(ids[3], ids[0], false).unwrap();
        assert_eq!(up.rules[0], original.rules[3]);
        assert_eq!(up.rules[1], original.rules[0]);
        assert_eq!(
            editor.candidate_rule_drop(ids[1], ids[2], false).unwrap(),
            original,
            "adjacent drop can be a no-op"
        );
        assert_eq!(
            editor.candidate_rule_drop(ids[1], ids[1], true).unwrap(),
            original
        );
        assert!(editor
            .candidate_rule_drop("missing", ids[0], false)
            .is_err());
        assert!(editor.candidate_rule_drop(ids[0], "missing", true).is_err());
        assert_eq!(
            editor.config, original,
            "only a successful save can commit the candidate"
        );
    }

    #[test]
    fn filtered_history_uses_original_ids_and_archive_total_ids() {
        let config = default_config();
        let mut runtime = initial_runtime();
        runtime.logs.push(LogEntry {
            id: "event-1".into(),
            at: 1_000,
            level: "success".into(),
            message: "Presence active".into(),
        });
        runtime.archive.push(ArchivedActivity {
            kind: "application".into(), archived_at: 2_000,
            entry: json!({"id":"total-generation-1","ruleId":"old-rule","name":"Editor","process":"Code.exe","durationMs":5000}),
            extra: Default::default(),
        });
        let rows = activity_rows(&config, &runtime, "Events", "ACTIVE", false, 10_000);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "event-1");
        assert!(activity_rows(&config, &runtime, "Sessions", "", false, 10_000).is_empty());
        let archive = activity_rows(&config, &runtime, "all", "code", true, 10_000);
        assert_eq!(archive[0].id, "total-generation-1");
        assert!(archive[0].archived);
        assert!(!archive[0].locked);
    }

    #[test]
    fn live_recording_adds_only_uncommitted_time_and_locks_management() {
        let config = default_config();
        let mut runtime = initial_runtime();
        runtime.usage_totals.insert("rule-1".into(), 10_000);
        runtime.usage_sessions.push(UsageSession {
            id: "session-1".into(),
            rule_id: "rule-1".into(),
            name: "Editor".into(),
            process: "Code.exe".into(),
            started_at: 1_000,
            ended_at: 3_000,
            duration_ms: 2_000,
            extra: Default::default(),
        });
        runtime.recording = Some(Recording {
            session_id: "session-1".into(),
            rule_id: "rule-1".into(),
            since: 3_000,
        });
        let rows = activity_rows(&config, &runtime, "all", "", false, 8_000);
        assert_eq!(rows[0].title, "Editor");
        assert_eq!(rows[0].duration_ms, 15_000);
        assert!(rows[0].locked);
        assert_eq!(rows[1].duration_ms, 7_000);
        assert!(rows[1].locked);
        let rows = activity_rows(&config, &runtime, "all", "", false, 2_000);
        assert_eq!(rows[0].duration_ms, 10_000);
        assert_eq!(rows[1].duration_ms, 2_000);
    }

    #[test]
    fn aliases_and_invalid_field_values_are_handled_without_panics() {
        let mut editor = editor();
        editor.set_field("large-image", "asset_key").unwrap();
        editor.set_field("app_name", "Name").unwrap();
        editor.set_field("elapsed", "false").unwrap();
        assert_eq!(editor.draft.large_image, "asset_key");
        assert_eq!(editor.draft.app_name, "Name");
        assert!(!editor.draft.elapsed);
        assert!(editor.set_field("type", "99").is_err());
        assert!(editor.set_field("elapsed", "perhaps").is_err());
        assert!(editor.set_field("id", "replacement").is_err());
        assert!(editor.remove_button(0).is_err());
        editor.add_button().unwrap();
        editor.add_button().unwrap();
        assert!(editor.add_button().is_err());
    }

    #[test]
    fn button_editor_keys_match_the_one_based_native_form() {
        let mut editor = editor();
        editor.set_field("button1Label", "First").unwrap();
        editor
            .set_field("button1Url", "https://example.com/first")
            .unwrap();
        editor.set_field("button2Label", "Second").unwrap();
        editor
            .set_field("button2Url", "https://example.com/second")
            .unwrap();
        assert_eq!(editor.draft.buttons.len(), 2);
        assert_eq!(editor.draft.buttons[0].label, "First");
        assert_eq!(editor.draft.buttons[1].label, "Second");
        assert!(editor.candidate_save().is_ok());
        editor.set_field("button0Label", "First alias").unwrap();
        assert_eq!(editor.draft.buttons[0].label, "First alias");
        assert_eq!(editor.draft.buttons[1].label, "Second");
    }

    #[test]
    fn history_displays_absolute_local_timestamps_without_changing_duration_values() {
        let timestamp = 1_600_000_000_000;
        let clock = format_event_time(timestamp);
        assert_eq!(clock.len(), 11);
        assert_eq!(&clock[2..3], ":");
        assert_eq!(&clock[5..6], ":");
        assert!(clock.ends_with(" AM") || clock.ends_with(" PM"));
        assert!(format_session_time(timestamp).contains("2020"));
        assert_eq!(format_session_time(i64::MAX), "Unknown time");

        let config = default_config();
        let mut runtime = initial_runtime();
        runtime.logs.push(LogEntry {
            id: "absolute-event".into(),
            at: timestamp,
            level: "success".into(),
            message: "Absolute event".into(),
        });
        runtime.usage_sessions.push(UsageSession {
            id: "absolute-session".into(),
            rule_id: "rule".into(),
            name: "Editor".into(),
            process: "Code.exe".into(),
            started_at: timestamp - 12_000,
            ended_at: timestamp,
            duration_ms: 12_000,
            extra: Default::default(),
        });
        let rows = activity_rows(&config, &runtime, "all", "", false, timestamp + 10_000);
        let later = activity_rows(&config, &runtime, "all", "", false, timestamp + 500_000);
        assert_eq!(
            rows, later,
            "Absolute historical labels remain stable as time advances"
        );
        assert_eq!(rows[0].time, clock);
        assert!(rows[0].detail.contains("2020"));
        assert!(rows[1].detail.contains("2020"));
        assert!(rows[1].detail.contains(" – "));
        assert_eq!(rows[1].time, "0:00:12");
        assert_eq!(rows[1].duration_ms, 12_000);
    }
}
