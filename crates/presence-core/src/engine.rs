//! Validation and activity rendering ported from shared/engine.ts.
use crate::model::*;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

const CONFIG_ERROR: &str = "Invalid configuration format.";
const USAGE_ERROR: &str = "Invalid application time data.";
const ARCHIVE_ERROR: &str = "Invalid activity archive.";
const TIMER_ERROR: &str = "Invalid playback timer settings.";
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

// ECMAScript trim and \s include the byte-order mark, but exclude U+0085.
fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

fn trim(value: &str) -> &str {
    value.trim_matches(js_whitespace)
}

fn js_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn render_text(value: &str, metadata: Option<&Metadata>) -> String {
    // Discord's bound in the original renderer is 128 UTF-16 code units.
    // Rust strings cannot contain an unpaired surrogate; lossily decoding a
    // sliced pair displays the same replacement glyph as a browser does.
    let units: Vec<u16> = resolve_template(value, metadata)
        .encode_utf16()
        .take(128)
        .collect();
    String::from_utf16_lossy(&units)
}

pub fn resolve_template(value: &str, metadata: Option<&Metadata>) -> String {
    static TEMPLATE: OnceLock<Regex> = OnceLock::new();
    let pattern = TEMPLATE.get_or_init(|| {
        Regex::new(concat!(
            r"\{\{[\x09-\x0d\x20\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000\ufeff]*",
            r"(app|title|process)",
            r"[\x09-\x0d\x20\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000\ufeff]*\}\}"
        ))
        .expect("constant template pattern is valid")
    });
    let rendered = pattern.replace_all(value, |captures: &regex::Captures<'_>| {
        metadata
            .map(|m| match &captures[1] {
                "app" => m.app.clone(),
                "title" => m.title.clone(),
                "process" => m.process.clone(),
                _ => unreachable!(),
            })
            .unwrap_or_default()
    });
    trim(&rendered).to_owned()
}

fn normalized_process(process: &str) -> String {
    let lowered = process.to_lowercase();
    lowered.strip_suffix(".exe").unwrap_or(&lowered).to_owned()
}

pub fn match_rule<'a>(
    rules: &'a [AppRule],
    apps: &'a [DetectedApp],
) -> Option<(&'a AppRule, &'a DetectedApp)> {
    for rule in rules.iter().filter(|rule| rule.enabled) {
        let expected = normalized_process(&rule.process);
        let title = rule.title_contains.to_lowercase();
        if let Some(app) = apps.iter().find(|app| {
            normalized_process(&app.process) == expected
                && app.title.to_lowercase().contains(&title)
        }) {
            return Some((rule, app));
        }
    }
    None
}

pub fn metadata_for(app: &DetectedApp) -> Metadata {
    let name = app.name.as_deref().filter(|name| !name.is_empty());
    let fallback = if app.process.to_lowercase().ends_with(".exe") {
        &app.process[..app.process.len() - 4]
    } else {
        &app.process
    };
    Metadata {
        app: name.unwrap_or(fallback).to_owned(),
        process: app.process.clone(),
        title: app.title.clone(),
    }
}

pub fn parse_playback_duration(value: &str) -> Option<i64> {
    let value = trim(value);
    let parts: Vec<&str> = value.split(':').collect();
    if parts.is_empty()
        || parts.len() > 3
        || parts[0].is_empty()
        || parts[0].len() > 6
        || !parts[0].bytes().all(|c| c.is_ascii_digit())
        || parts.iter().skip(1).any(|part| {
            part.len() != 2
                || !part.bytes().all(|c| c.is_ascii_digit())
                || part.as_bytes()[0] > b'5'
        })
    {
        return None;
    }
    let seconds = parts
        .iter()
        .try_fold(0_i64, |total, part| {
            part.parse::<i64>().map(|n| total * 60 + n)
        })
        .ok()?;
    (seconds > 0 && seconds <= 604_800).then_some(seconds)
}

pub fn playback_duration_seconds(profile: &Profile) -> Option<i64> {
    if profile.elapsed && profile.progress && matches!(profile.r#type, 2 | 3) {
        parse_playback_duration(&profile.duration)
    } else {
        None
    }
}

pub fn application_id_for<'a>(profile: &'a Profile, global_application_id: &'a str) -> &'a str {
    if profile.application_id.is_empty() {
        global_application_id
    } else {
        &profile.application_id
    }
}

pub fn valid_application_id(value: &str) -> bool {
    (17..=20).contains(&value.len()) && value.bytes().all(|c| c.is_ascii_digit())
}

pub fn validate_profile(
    profile: &Profile,
    publishing: bool,
    global_application_id: &str,
) -> Result<(), String> {
    if trim(&profile.name).is_empty() {
        return Err("Give your presence a name.".into());
    }
    if js_len(&profile.name) > 60 || js_len(&profile.app_name) > 128 {
        return Err("Presence names are limited to 60 characters and display names to 128.".into());
    }
    if !profile.application_id.is_empty() && !valid_application_id(&profile.application_id) {
        return Err("Application IDs contain 17–20 digits.".into());
    }
    if publishing && !valid_application_id(application_id_for(profile, global_application_id)) {
        return Err("Add a valid Discord application ID in Settings before going live.".into());
    }
    if !matches!(profile.r#type, 0 | 2 | 3 | 5) {
        return Err("Choose a supported activity type.".into());
    }
    if js_len(&profile.duration) > 12 {
        return Err(TIMER_ERROR.into());
    }
    if profile.progress && parse_playback_duration(&profile.duration).is_none() {
        return Err(
            "Enter a playback duration from 1 second to 7 days, using m:ss or h:mm:ss.".into(),
        );
    }
    if profile.buttons.len() > 2 {
        return Err("Discord supports up to two buttons.".into());
    }
    for button in &profile.buttons {
        if trim(&button.label).is_empty() || js_len(&button.label) > 32 {
            return Err("Button labels must contain 1–32 characters.".into());
        }
        let url = url::Url::parse(&button.url)
            .map_err(|_| "Each button needs a valid HTTPS or HTTP URL.".to_owned())?;
        if !matches!(url.scheme(), "https" | "http") {
            return Err("Button links must use HTTPS or HTTP.".into());
        }
        if js_len(&button.url) > 512 {
            return Err("Button links are limited to 512 characters.".into());
        }
    }
    for image in [&profile.large_image, &profile.small_image] {
        if js_len(image) > 512 {
            return Err("Image values are limited to 512 characters.".into());
        }
        if image.contains("://") && !image.starts_with("https://") {
            return Err("Artwork URLs must use HTTPS.".into());
        }
    }
    for text in [
        &profile.details,
        &profile.state,
        &profile.large_text,
        &profile.small_text,
    ] {
        if js_len(text) > 128 {
            return Err("Presence text is limited to 128 characters per field.".into());
        }
    }
    Ok(())
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_owned)
}

fn has_strings(value: &Value, keys: &[&str]) -> bool {
    keys.iter()
        .all(|key| value.get(*key).is_some_and(Value::is_string))
}

fn extra_fields(value: &Value, known: &[&str]) -> Map<String, Value> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter(|(key, _)| !known.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn safe_nonnegative_integer(value: &Value) -> Option<i64> {
    // JSON numbers such as 1000.0 are valid JavaScript safe integers too.
    if let Some(number) = value.as_i64() {
        return (0..=MAX_SAFE_INTEGER).contains(&number).then_some(number);
    }
    let number = value.as_f64()?;
    (number.is_finite()
        && number >= 0.0
        && number <= MAX_SAFE_INTEGER as f64
        && number.fract() == 0.0)
        .then_some(number as i64)
}

pub fn validate_config(input: &Value) -> Result<Config, String> {
    if !input.is_object() && !input.is_array() {
        return Err("This is not a Presence Control configuration.".into());
    }
    let profiles = input.get("profiles").and_then(Value::as_array);
    let rules = input.get("rules").and_then(Value::as_array);
    if input.get("version").and_then(Value::as_f64) != Some(1.0)
        || profiles.is_none_or(|v| v.is_empty() || v.len() > 100)
        || rules.is_none_or(|v| v.len() > 100)
        || input.get("automatic").and_then(Value::as_bool).is_none()
        || input.get("recordTime").is_some_and(|v| !v.is_boolean())
    {
        return Err(CONFIG_ERROR.into());
    }
    let application_id = match input.get("applicationId") {
        None => String::new(),
        Some(value) => value
            .as_str()
            .filter(|id| id.is_empty() || valid_application_id(id))
            .ok_or("The global Discord application ID must contain 17–20 digits.")?
            .to_owned(),
    };
    let mut ids = HashSet::new();
    let mut normalized_profiles = Vec::new();
    for profile in profiles.unwrap() {
        if !has_strings(
            profile,
            &[
                "id",
                "name",
                "applicationId",
                "appName",
                "details",
                "state",
                "largeImage",
                "largeText",
                "smallImage",
                "smallText",
            ],
        ) || profile.get("elapsed").and_then(Value::as_bool).is_none()
            || !matches!(
                profile.get("theme").and_then(Value::as_str),
                Some("violet" | "mint" | "peach" | "blue")
            )
            || profile.get("buttons").and_then(Value::as_array).is_none()
        {
            return Err("Invalid presence profile.".into());
        }
        let id = string_field(profile, "id").unwrap();
        if id.is_empty() || !ids.insert(id.clone()) {
            return Err("Profiles need unique IDs.".into());
        }
        let mut buttons = Vec::new();
        for button in profile["buttons"].as_array().unwrap() {
            if !has_strings(button, &["label", "url"]) {
                return Err("Invalid button.".into());
            }
            buttons.push(Button {
                label: string_field(button, "label").unwrap(),
                url: string_field(button, "url").unwrap(),
                extra: extra_fields(button, &["label", "url"]),
            });
        }
        let progress = match profile.get("progress") {
            None => false,
            Some(value) => value.as_bool().ok_or(TIMER_ERROR)?,
        };
        let duration = match profile.get("duration") {
            None => "3:00".to_owned(),
            Some(value) => value.as_str().ok_or(TIMER_ERROR)?.to_owned(),
        };
        let activity_type = profile
            .get("type")
            .and_then(safe_nonnegative_integer)
            .and_then(|n| u8::try_from(n).ok())
            .unwrap_or(u8::MAX);
        let normalized = Profile {
            id,
            name: string_field(profile, "name").unwrap(),
            application_id: string_field(profile, "applicationId").unwrap(),
            app_name: string_field(profile, "appName").unwrap(),
            r#type: activity_type,
            details: string_field(profile, "details").unwrap(),
            state: string_field(profile, "state").unwrap(),
            large_image: string_field(profile, "largeImage").unwrap(),
            large_text: string_field(profile, "largeText").unwrap(),
            small_image: string_field(profile, "smallImage").unwrap(),
            small_text: string_field(profile, "smallText").unwrap(),
            elapsed: profile["elapsed"].as_bool().unwrap(),
            progress,
            duration,
            theme: string_field(profile, "theme").unwrap(),
            buttons,
            extra: extra_fields(
                profile,
                &[
                    "id",
                    "name",
                    "applicationId",
                    "appName",
                    "type",
                    "details",
                    "state",
                    "largeImage",
                    "largeText",
                    "smallImage",
                    "smallText",
                    "elapsed",
                    "progress",
                    "duration",
                    "theme",
                    "buttons",
                ],
            ),
        };
        validate_profile(&normalized, false, "")?;
        normalized_profiles.push(normalized);
    }
    let mut rule_ids = HashSet::new();
    let mut normalized_rules = Vec::new();
    for rule in rules.unwrap() {
        if !has_strings(
            rule,
            &["id", "name", "process", "titleContains", "profileId"],
        ) || rule.get("enabled").and_then(Value::as_bool).is_none()
        {
            return Err("Invalid application rule.".into());
        }
        let rule = AppRule {
            id: string_field(rule, "id").unwrap(),
            name: string_field(rule, "name").unwrap(),
            process: string_field(rule, "process").unwrap(),
            title_contains: string_field(rule, "titleContains").unwrap(),
            profile_id: string_field(rule, "profileId").unwrap(),
            enabled: rule["enabled"].as_bool().unwrap(),
            extra: extra_fields(
                rule,
                &[
                    "id",
                    "name",
                    "process",
                    "titleContains",
                    "profileId",
                    "enabled",
                ],
            ),
        };
        if trim(&rule.process).is_empty()
            || !ids.contains(&rule.profile_id)
            || !rule_ids.insert(rule.id.clone())
        {
            return Err("Invalid application rule.".into());
        }
        normalized_rules.push(rule);
    }
    Ok(Config {
        version: 1,
        application_id,
        profiles: normalized_profiles,
        rules: normalized_rules,
        automatic: input["automatic"].as_bool().unwrap(),
        record_time: input
            .get("recordTime")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        extra: extra_fields(
            input,
            &[
                "version",
                "applicationId",
                "profiles",
                "rules",
                "automatic",
                "recordTime",
            ],
        ),
    })
}

fn validate_usage_session(input: &Value) -> Result<UsageSession, String> {
    if !has_strings(input, &["id", "ruleId", "name", "process"]) {
        return Err(USAGE_ERROR.into());
    }
    let session = UsageSession {
        id: string_field(input, "id").unwrap(),
        rule_id: string_field(input, "ruleId").unwrap(),
        name: string_field(input, "name").unwrap(),
        process: string_field(input, "process").unwrap(),
        started_at: input
            .get("startedAt")
            .and_then(safe_nonnegative_integer)
            .ok_or(USAGE_ERROR)?,
        ended_at: input
            .get("endedAt")
            .and_then(safe_nonnegative_integer)
            .ok_or(USAGE_ERROR)?,
        duration_ms: input
            .get("durationMs")
            .and_then(safe_nonnegative_integer)
            .ok_or(USAGE_ERROR)?,
        extra: extra_fields(
            input,
            &[
                "id",
                "ruleId",
                "name",
                "process",
                "startedAt",
                "endedAt",
                "durationMs",
            ],
        ),
    };
    if session.id.is_empty()
        || session.rule_id.is_empty()
        || session.ended_at < session.started_at
        || session.duration_ms > session.ended_at - session.started_at
    {
        return Err(USAGE_ERROR.into());
    }
    Ok(session)
}

pub fn validate_usage_data(input: &Value) -> Result<UsageData, String> {
    let raw_totals = input
        .get("totals")
        .and_then(Value::as_object)
        .ok_or(USAGE_ERROR)?;
    let raw_sessions = input
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or(USAGE_ERROR)?;
    if !input.is_object() || raw_sessions.len() > 500 {
        return Err(USAGE_ERROR.into());
    }
    let mut totals = BTreeMap::new();
    for (id, duration) in raw_totals {
        if id.is_empty() {
            return Err(USAGE_ERROR.into());
        }
        totals.insert(
            id.clone(),
            safe_nonnegative_integer(duration).ok_or(USAGE_ERROR)?,
        );
    }
    let mut sessions = Vec::new();
    let mut session_ids = HashSet::new();
    for session in raw_sessions {
        let session = validate_usage_session(session)?;
        if !session_ids.insert(session.id.clone()) {
            return Err(USAGE_ERROR.into());
        }
        sessions.push(session);
    }
    let total_ids = if let Some(raw_ids) = input.get("totalIds") {
        let mut result = BTreeMap::new();
        let mut unique = HashSet::new();
        for (rule_id, total_id) in raw_ids.as_object().ok_or(USAGE_ERROR)? {
            let total_id = total_id.as_str().ok_or(USAGE_ERROR)?;
            if !totals.contains_key(rule_id) || total_id.is_empty() || !unique.insert(total_id) {
                return Err(USAGE_ERROR.into());
            }
            result.insert(rule_id.clone(), total_id.to_owned());
        }
        Some(result)
    } else {
        None
    };
    Ok(UsageData {
        totals,
        sessions,
        total_ids,
    })
}

pub fn validate_archive_data(input: &Value) -> Result<ArchiveData, String> {
    let items = input
        .get("entries")
        .and_then(Value::as_array)
        .ok_or(ARCHIVE_ERROR)?;
    let mut ids = HashSet::new();
    let mut entries = Vec::new();
    for item in items {
        let kind = item
            .get("kind")
            .and_then(Value::as_str)
            .ok_or(ARCHIVE_ERROR)?;
        if !matches!(kind, "event" | "session" | "application") {
            return Err(ARCHIVE_ERROR.into());
        }
        let archived_at = item
            .get("archivedAt")
            .and_then(safe_nonnegative_integer)
            .ok_or(ARCHIVE_ERROR)?;
        let entry = item.get("entry").ok_or(ARCHIVE_ERROR)?;
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(ARCHIVE_ERROR)?;
        if !ids.insert((kind, id)) {
            return Err(ARCHIVE_ERROR.into());
        }
        match kind {
            "session" => {
                validate_usage_session(entry)?;
            }
            "application" => {
                if !has_strings(entry, &["ruleId", "name", "process"])
                    || entry["ruleId"].as_str() == Some("")
                    || entry
                        .get("durationMs")
                        .and_then(safe_nonnegative_integer)
                        .is_none()
                {
                    return Err("Invalid application time archive.".into());
                }
            }
            "event" => {
                if entry.get("at").and_then(safe_nonnegative_integer).is_none()
                    || !matches!(
                        entry.get("level").and_then(Value::as_str),
                        Some("info" | "success" | "error")
                    )
                    || !entry.get("message").is_some_and(Value::is_string)
                {
                    return Err(ARCHIVE_ERROR.into());
                }
            }
            _ => unreachable!(),
        }
        entries.push(ArchivedActivity {
            kind: kind.to_owned(),
            entry: entry.clone(),
            archived_at,
            extra: extra_fields(item, &["kind", "entry", "archivedAt"]),
        });
    }
    Ok(ArchiveData { entries })
}

pub fn build_activity(profile: &Profile, metadata: Option<&Metadata>, since: i64) -> Value {
    let mut activity = Map::new();
    if !trim(&profile.app_name).is_empty() {
        activity.insert(
            "name".into(),
            render_text(&profile.app_name, metadata).into(),
        );
    }
    activity.insert("type".into(), profile.r#type.into());
    for (key, value) in [("details", &profile.details), ("state", &profile.state)] {
        let rendered = render_text(value, metadata);
        if !rendered.is_empty() {
            activity.insert(key.into(), rendered.into());
        }
    }
    if profile.elapsed {
        let start = since.div_euclid(1000);
        let mut timestamps = json!({ "start": start });
        if let Some(duration) = playback_duration_seconds(profile) {
            timestamps["end"] = (start + duration).into();
        }
        activity.insert("timestamps".into(), timestamps);
    }
    let mut assets = Map::new();
    for (key, value) in [
        ("large_image", profile.large_image.clone()),
        ("large_text", render_text(&profile.large_text, metadata)),
        ("small_image", profile.small_image.clone()),
        ("small_text", render_text(&profile.small_text, metadata)),
    ] {
        if !value.is_empty() {
            assets.insert(key.into(), value.into());
        }
    }
    if !assets.is_empty() {
        activity.insert("assets".into(), assets.into());
    }
    if !profile.buttons.is_empty() {
        let buttons: Vec<Value> = profile
            .buttons
            .iter()
            .map(|b| json!({ "label": trim(&b.label), "url": b.url }))
            .collect();
        activity.insert("buttons".into(), buttons.into());
    }
    activity.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_json() -> Value {
        serde_json::to_value(default_config()).unwrap()
    }

    fn session_json() -> Value {
        json!({
            "id": "session", "ruleId": "editor", "name": "Editor", "process": "Code.exe",
            "startedAt": 1000, "endedAt": 61000, "durationMs": 60000
        })
    }

    #[test]
    fn templates_preserve_unicode_unknown_tokens_and_literal_content() {
        let meta = Metadata {
            app: "Editor".into(),
            title: "日本語 — <script>alert(1)</script> $1".into(),
            process: "Code.exe".into(),
        };
        assert_eq!(
            resolve_template("Working in {{ app }}: {{title}}", Some(&meta)),
            format!("Working in Editor: {}", meta.title)
        );
        assert_eq!(
            resolve_template("{{process}} {{unknown}}", Some(&meta)),
            "Code.exe {{unknown}}"
        );
        assert_eq!(resolve_template("{{title}}", None), "");
        assert_eq!(
            resolve_template("\u{feff}{{\u{feff}app\u{feff}}}\u{feff}", Some(&meta)),
            "Editor"
        );
        assert_eq!(
            resolve_template("{{\u{0085}app}}", Some(&meta)),
            "{{\u{0085}app}}"
        );
    }

    #[test]
    fn rules_match_exact_process_names_in_priority_order() {
        let first = AppRule {
            id: "one".into(),
            name: "Editor".into(),
            process: "CODE".into(),
            title_contains: "Presence".into(),
            profile_id: "creative-flow".into(),
            enabled: true,
            extra: Default::default(),
        };
        let second = AppRule {
            id: "two".into(),
            ..first.clone()
        };
        let apps = vec![
            DetectedApp {
                pid: 1,
                process: "Code.exe".into(),
                title: "presence.ts — Code".into(),
                name: None,
            },
            DetectedApp {
                pid: 2,
                process: "CodeHelper.exe".into(),
                title: "Presence".into(),
                name: None,
            },
        ];
        let rules = [first.clone(), second];
        let (rule, app) = match_rule(&rules, &apps).unwrap();
        assert_eq!(rule.id, "one");
        assert_eq!(app.pid, 1);
        for patch in [
            AppRule {
                process: "Cod".into(),
                ..first.clone()
            },
            AppRule {
                enabled: false,
                ..first.clone()
            },
            AppRule {
                title_contains: "Other".into(),
                ..first.clone()
            },
        ] {
            assert!(match_rule(&[patch], &apps).is_none());
        }
    }

    #[test]
    fn application_metadata_prefers_product_name_and_preserves_process_case() {
        let mut app = DetectedApp {
            pid: 1,
            process: "Code.EXE".into(),
            title: "File".into(),
            name: Some("Visual Studio Code".into()),
        };
        assert_eq!(metadata_for(&app).app, "Visual Studio Code");
        app.name = None;
        assert_eq!(metadata_for(&app).app, "Code");
        app.name = Some(String::new());
        assert_eq!(metadata_for(&app).app, "Code");
        assert_eq!(metadata_for(&app).process, "Code.EXE");
    }

    #[test]
    fn activities_render_bounds_assets_buttons_and_elapsed_timers() {
        let mut profile = default_config().profiles.remove(0);
        profile.details = "{{title}}".into();
        profile.large_image = "cover".into();
        profile.buttons = vec![Button {
            label: " Site ".into(),
            url: "https://example.com".into(),
            extra: Default::default(),
        }];
        let meta = Metadata {
            app: "App".into(),
            title: "a".repeat(300),
            process: "app.exe".into(),
        };
        let activity = build_activity(&profile, Some(&meta), 1_720_000_000_123);
        assert_eq!(activity["details"].as_str().unwrap().len(), 128);
        assert_eq!(activity["timestamps"], json!({ "start": 1_720_000_000 }));
        assert_eq!(activity["assets"]["large_image"], "cover");
        assert_eq!(
            activity["buttons"],
            json!([{ "label": "Site", "url": "https://example.com" }])
        );
        profile.elapsed = false;
        assert!(build_activity(&profile, Some(&meta), 0)
            .get("timestamps")
            .is_none());
        profile.details = "{{unknown}}".into();
        assert_eq!(build_activity(&profile, None, 0)["details"], "{{unknown}}");
        profile.details = "{{title}}".into();
        assert!(build_activity(&profile, None, 0).get("details").is_none());
    }

    #[test]
    fn media_progress_applies_to_listening_and_watching_only() {
        let mut profile = default_config().profiles.remove(0);
        profile.r#type = 2;
        profile.app_name = "Music player".into();
        profile.details = "Track: {{title}}".into();
        profile.state = "Artist name".into();
        profile.large_text = "Album name".into();
        profile.progress = true;
        profile.duration = "4:30".into();
        let metadata = Metadata {
            app: "Player".into(),
            title: "Song".into(),
            process: "player.exe".into(),
        };
        let activity = build_activity(&profile, Some(&metadata), 1_720_000_000_123);
        assert_eq!(activity["name"], "Music player");
        assert_eq!(activity["details"], "Track: Song");
        assert_eq!(activity["state"], "Artist name");
        assert_eq!(activity["assets"]["large_text"], "Album name");
        assert_eq!(
            activity["timestamps"],
            json!({ "start": 1_720_000_000, "end": 1_720_000_270 })
        );
        profile.r#type = 3;
        assert_eq!(
            build_activity(&profile, None, 1_720_000_000_123)["timestamps"],
            activity["timestamps"]
        );
        profile.r#type = 0;
        assert_eq!(
            build_activity(&profile, None, 1_720_000_000_123)["timestamps"],
            json!({ "start": 1_720_000_000 })
        );
        profile.r#type = 2;
        profile.elapsed = false;
        assert!(build_activity(&profile, None, 0)
            .get("timestamps")
            .is_none());
        profile.elapsed = true;
        profile.progress = false;
        assert!(build_activity(&profile, None, 0)["timestamps"]
            .get("end")
            .is_none());
    }

    #[test]
    fn playback_duration_accepts_seconds_minutes_hours_and_seven_day_limit() {
        for (text, seconds) in [
            ("210", 210),
            ("3:30", 210),
            ("1:02:03", 3723),
            ("0:01", 1),
            (" 90:30 ", 5430),
            ("168:00:00", 604800),
        ] {
            assert_eq!(parse_playback_duration(text), Some(seconds), "{text}");
        }
        for text in [
            "",
            "0",
            "-1",
            "1:60",
            "1:2",
            "1:00:60",
            "3.5",
            "NaN",
            "999999",
            "1:00:00:00",
            "١٢",
            "168:00:01",
        ] {
            assert_eq!(parse_playback_duration(text), None, "{text}");
        }
        let mut profile = default_config().profiles.remove(0);
        profile.progress = true;
        for duration in ["0:00", "3:90"] {
            profile.duration = duration.into();
            assert!(validate_profile(&profile, false, "")
                .unwrap_err()
                .contains("duration"));
        }
    }

    #[test]
    fn config_round_trip_and_legacy_fields_preserve_defaults() {
        let expected = default_config();
        assert_eq!(
            validate_config(&serde_json::to_value(&expected).unwrap()).unwrap(),
            expected
        );
        let mut legacy = config_json();
        legacy.as_object_mut().unwrap().remove("applicationId");
        legacy.as_object_mut().unwrap().remove("recordTime");
        for profile in legacy["profiles"].as_array_mut().unwrap() {
            profile.as_object_mut().unwrap().remove("progress");
            profile.as_object_mut().unwrap().remove("duration");
        }
        let migrated = validate_config(&legacy).unwrap();
        assert_eq!(migrated.application_id, "");
        assert!(!migrated.record_time);
        assert!(migrated.profiles[0].elapsed);
        assert!(!migrated.profiles[0].progress);
        assert_eq!(migrated.profiles[0].duration, "3:00");
        for patch in [
            json!({"progress": "yes"}),
            json!({"progress": null}),
            json!({"duration": 123}),
            json!({"duration": null}),
        ] {
            let mut invalid = legacy.clone();
            invalid["profiles"][0]
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(validate_config(&invalid).unwrap_err().contains("timer"));
        }
    }

    #[test]
    fn config_rejects_bad_identity_references_and_malformed_imports() {
        let mut config = config_json();
        config["profiles"] = json!([config["profiles"][0], config["profiles"][0]]);
        assert!(validate_config(&config).unwrap_err().contains("unique"));
        config = config_json();
        config["rules"] = json!([{
            "id":"r", "name":"X", "process":"x.exe", "titleContains":"",
            "profileId":"missing", "enabled":true
        }]);
        assert!(validate_config(&config).unwrap_err().contains("rule"));
        config["rules"][0]["profileId"] = "creative-flow".into();
        assert!(validate_config(&config).is_ok());
        config["rules"][0]["process"] = " ".into();
        assert!(validate_config(&config).is_err());
        assert!(validate_config(
            &json!({"version":1,"profiles":[{"name":123}],"rules":[],"automatic":true})
        )
        .unwrap_err()
        .contains("profile"));
        for bad in [Value::Null, json!("config"), json!({}), json!([])] {
            assert!(validate_config(&bad).is_err());
        }
        config = config_json();
        config["recordTime"] = "true".into();
        assert!(validate_config(&config).unwrap_err().contains("format"));
    }

    #[test]
    fn publishing_uses_global_application_id_and_profile_override() {
        let mut config = config_json();
        for application_id in [
            Value::Null,
            json!(123),
            json!(false),
            json!("abc"),
            json!("123"),
            json!("123456789012345678901"),
        ] {
            config["applicationId"] = application_id;
            assert!(validate_config(&config)
                .unwrap_err()
                .contains("global Discord application ID"));
        }
        config["applicationId"] = "987654321098765432".into();
        let mut validated = validate_config(&config).unwrap();
        assert!(validate_profile(&validated.profiles[0], true, &validated.application_id).is_ok());
        assert!(validate_profile(&validated.profiles[0], true, "")
            .unwrap_err()
            .contains("application ID"));
        validated.profiles[0].application_id = "bad".into();
        assert!(
            validate_profile(&validated.profiles[0], true, &validated.application_id)
                .unwrap_err()
                .contains("Application IDs")
        );
    }

    #[test]
    fn profile_validation_bounds_utf16_text_and_http_urls() {
        let mut profile = default_config().profiles.remove(0);
        profile.buttons = vec![Button {
            label: "x".into(),
            url: "javascript:alert(1)".into(),
            extra: Default::default(),
        }];
        assert!(validate_profile(&profile, false, "")
            .unwrap_err()
            .contains("HTTPS"));
        profile.buttons[0].url = "relative/path".into();
        assert!(validate_profile(&profile, false, "")
            .unwrap_err()
            .contains("valid"));
        profile.buttons[0].url = "http://example.com".into();
        assert!(validate_profile(&profile, false, "").is_ok());
        profile.large_image = "http://example.com/art.png".into();
        assert!(validate_profile(&profile, false, "")
            .unwrap_err()
            .contains("Artwork"));
        profile.large_image = "asset-key".into();
        profile.name = "😀".repeat(30);
        assert!(validate_profile(&profile, false, "").is_ok());
        profile.name.push('a');
        assert!(validate_profile(&profile, false, "")
            .unwrap_err()
            .contains("60 characters"));
        profile.name = "Valid".into();
        profile.details = "😀".repeat(65);
        assert!(validate_profile(&profile, false, "")
            .unwrap_err()
            .contains("128 characters"));
    }

    #[test]
    fn usage_validates_sessions_safe_integers_and_total_identities() {
        let valid = json!({"totals":{"editor":60000},"sessions":[session_json()],"totalIds":{"editor":"total-1"}});
        assert_eq!(
            validate_usage_data(&valid).unwrap().total_ids.unwrap()["editor"],
            "total-1"
        );
        for duration in [
            json!(-1),
            json!("1000"),
            json!(1.5),
            json!(9_007_199_254_740_992_i64),
        ] {
            assert!(
                validate_usage_data(&json!({"totals":{"editor":duration},"sessions":[]})).is_err()
            );
        }
        assert!(validate_usage_data(&json!({"totals":{"editor":1000.0},"sessions":[]})).is_ok());
        assert!(validate_usage_data(
            &json!({"totals":{},"sessions":[session_json(), session_json()]})
        )
        .is_err());
        let mut session = session_json();
        session["endedAt"] = 0.into();
        assert!(validate_usage_data(&json!({"totals":{},"sessions":[session]})).is_err());
        for total_ids in [
            Value::Null,
            json!([]),
            json!("bad"),
            json!({"missing":"total"}),
            json!({"editor":""}),
            json!({"editor":1}),
        ] {
            assert!(validate_usage_data(
                &json!({"totals":{"editor":10000},"sessions":[],"totalIds":total_ids})
            )
            .is_err());
        }
        assert!(validate_usage_data(&json!({"totals":{"editor":1,"notes":1},"sessions":[],"totalIds":{"editor":"same","notes":"same"}})).is_err());
        // JavaScript's prototype-related property names remain ordinary map keys.
        let usage =
            validate_usage_data(&json!({"totals":{"__proto__":1,"constructor":2},"sessions":[]}))
                .unwrap();
        assert_eq!(usage.totals["__proto__"], 1);
        assert_eq!(usage.totals["constructor"], 2);
    }

    #[test]
    fn archives_preserve_all_kinds_without_recent_history_retention_limit() {
        let event = json!({"kind":"event","archivedAt":20000,"entry":{"id":"one","at":1000,"level":"info","message":"Saved event"}});
        let session = json!({"kind":"session","archivedAt":20000,"entry":session_json()});
        let application = json!({"kind":"application","archivedAt":20000,"entry":{"id":"total-1","ruleId":"editor","name":"Editor","process":"Code.exe","durationMs":10000}});
        assert_eq!(
            validate_archive_data(
                &json!({"entries":[event.clone(),session.clone(),application.clone()]})
            )
            .unwrap()
            .entries
            .len(),
            3
        );
        let many: Vec<Value> = (0..600)
            .map(|i| {
                let mut item = session.clone();
                item["entry"]["id"] = i.to_string().into();
                item
            })
            .collect();
        assert_eq!(
            validate_archive_data(&json!({"entries":many}))
                .unwrap()
                .entries
                .len(),
            600
        );
        assert!(
            validate_usage_data(&json!({"totals":{},"sessions":vec![session_json();501]})).is_err()
        );
        for bad in [
            Value::Null,
            json!({}),
            json!({"entries":[null]}),
            json!({"entries":[event.clone(),event.clone()]}),
        ] {
            assert!(validate_archive_data(&bad).is_err());
        }
        for patch in [
            json!({"durationMs":-1}),
            json!({"durationMs":1.5}),
            json!({"ruleId":""}),
            json!({"process":null}),
        ] {
            let mut invalid = application.clone();
            invalid["entry"]
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(validate_archive_data(&json!({"entries":[invalid]})).is_err());
        }
        for patch in [
            json!({"kind":"unknown"}),
            json!({"archivedAt":-1}),
            json!({"entry":{"id":"one","at":1000,"level":"unknown","message":"Bad"}}),
        ] {
            let mut invalid = event.clone();
            invalid
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(validate_archive_data(&json!({"entries":[invalid]})).is_err());
        }
    }

    #[test]
    fn runtime_contract_serializes_nulls_and_camel_case() {
        let runtime = serde_json::to_value(initial_runtime()).unwrap();
        assert_eq!(runtime["connection"], "disconnected");
        assert_eq!(runtime["activeProfileId"], Value::Null);
        assert_eq!(runtime["activeRuleId"], Value::Null);
        assert_eq!(runtime["recording"], Value::Null);
        assert_eq!(runtime["usageTotals"], json!({}));
        assert_eq!(runtime["usageSessions"], json!([]));
        assert!(runtime.get("active_profile_id").is_none());
        let app = serde_json::to_value(DetectedApp {
            pid: 1,
            process: "x".into(),
            title: "".into(),
            name: None,
        })
        .unwrap();
        assert!(app.get("name").is_none());
    }

    #[test]
    fn imports_preserve_unknown_extension_fields_like_the_original_object_spreads() {
        let mut config = config_json();
        config["extension"] = json!({"enabled": true});
        config["profiles"][0]["extension"] = json!(["profile"]);
        config["profiles"][0]["buttons"] = json!([{
            "label": "Site", "url": "https://example.com", "extension": "button"
        }]);
        config["rules"] = json!([{
            "id":"editor", "name":"Editor", "process":"Code.exe", "titleContains":"",
            "profileId":"creative-flow", "enabled":true, "extension":"rule"
        }]);
        let validated = validate_config(&config).unwrap();
        assert_eq!(serde_json::to_value(&validated).unwrap(), config);
        let mut session = session_json();
        session["extension"] = json!({"session": true});
        let usage = json!({"totals":{},"sessions":[session.clone()]});
        assert_eq!(
            serde_json::to_value(validate_usage_data(&usage).unwrap()).unwrap(),
            usage
        );
        let archive = json!({"entries":[{
            "kind":"session", "entry":session, "archivedAt":20000, "extension":"archive"
        }]});
        assert_eq!(
            serde_json::to_value(validate_archive_data(&archive).unwrap()).unwrap(),
            archive
        );
        // Button extension properties belong in saved configuration, not the
        // outbound Discord activity, which maps only label and URL.
        assert!(
            build_activity(&validated.profiles[0], None, 0)["buttons"][0]
                .get("extension")
                .is_none()
        );
    }
}
