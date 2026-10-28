//! The serialized desktop API contract shared with the existing renderer.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type UsageTotals = BTreeMap<String, i64>;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Button {
    pub label: String,
    pub url: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub application_id: String,
    pub app_name: String,
    pub r#type: u8,
    pub details: String,
    pub state: String,
    pub large_image: String,
    pub large_text: String,
    pub small_image: String,
    pub small_text: String,
    pub elapsed: bool,
    pub progress: bool,
    pub duration: String,
    pub theme: String,
    pub buttons: Vec<Button>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRule {
    pub id: String,
    pub name: String,
    pub process: String,
    pub title_contains: String,
    pub profile_id: String,
    pub enabled: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    Dark,
    Light,
    System,
}

impl Appearance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::System => "system",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub version: u8,
    pub application_id: String,
    pub profiles: Vec<Profile>,
    pub rules: Vec<AppRule>,
    pub automatic: bool,
    pub record_time: bool,
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSession {
    pub id: String,
    pub rule_id: String,
    pub name: String,
    pub process: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_ms: i64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageData {
    pub totals: UsageTotals,
    pub sessions: Vec<UsageSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_ids: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationTime {
    pub id: String,
    pub rule_id: String,
    pub name: String,
    pub process: String,
    pub duration_ms: i64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedApp {
    pub pid: u32,
    pub process: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub app: String,
    pub title: String,
    pub process: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: String,
    pub at: i64,
    pub level: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub kind: String,
    pub entry: Value,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedActivity {
    pub kind: String,
    pub entry: Value,
    pub archived_at: i64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl ArchivedActivity {
    /// Older application archives remain fixed snapshots. New archives opt in
    /// to following the current total's generation until that total is deleted.
    pub fn tracks_application_time(&self) -> bool {
        self.kind == "application"
            && self
                .extra
                .get("tracksApplicationTime")
                .and_then(Value::as_bool)
                == Some(true)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveData {
    pub entries: Vec<ArchivedActivity>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    pub session_id: String,
    pub rule_id: String,
    pub since: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeState {
    pub connection: String,
    pub active_profile_id: Option<String>,
    pub active_rule_id: Option<String>,
    pub mode: Option<String>,
    pub metadata: Option<Metadata>,
    pub since: Option<i64>,
    pub apps: Vec<DetectedApp>,
    pub logs: Vec<LogEntry>,
    pub archive: Vec<ArchivedActivity>,
    pub error: Option<String>,
    pub user: Option<String>,
    pub usage_totals: UsageTotals,
    pub usage_sessions: Vec<UsageSession>,
    pub recording: Option<Recording>,
}

pub fn initial_runtime() -> RuntimeState {
    RuntimeState {
        connection: "disconnected".into(),
        active_profile_id: None,
        active_rule_id: None,
        mode: None,
        metadata: None,
        since: None,
        apps: Vec::new(),
        logs: Vec::new(),
        archive: Vec::new(),
        error: None,
        user: None,
        usage_totals: BTreeMap::new(),
        usage_sessions: Vec::new(),
        recording: None,
    }
}

pub fn create_profile(name: &str) -> Profile {
    Profile {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        application_id: String::new(),
        app_name: String::new(),
        r#type: 0,
        details: String::new(),
        state: String::new(),
        large_image: String::new(),
        large_text: String::new(),
        small_image: String::new(),
        small_text: String::new(),
        elapsed: true,
        progress: false,
        duration: "3:00".into(),
        theme: "violet".into(),
        buttons: Vec::new(),
        extra: Default::default(),
    }
}

pub fn default_config() -> Config {
    let mut creative = create_profile("Creative flow");
    creative.id = "creative-flow".into();
    creative.app_name = "Creative flow".into();
    creative.details = "Making something out of nothing".into();
    creative.state = "A little focus. A lot of possibility.".into();
    creative.large_text = "In the zone".into();
    let mut deep = create_profile("Deep work");
    deep.id = "deep-work".into();
    deep.app_name = "Deep work".into();
    deep.details = "One thing at a time".into();
    deep.state = "Notifications off. Ideas on.".into();
    deep.theme = "mint".into();
    let mut repeat = create_profile("On repeat");
    repeat.id = "on-repeat".into();
    repeat.app_name = "On repeat".into();
    repeat.r#type = 2;
    repeat.details = "A soundtrack for the moment".into();
    repeat.state = "Finding my next favorite".into();
    repeat.theme = "peach".into();
    Config {
        version: 1,
        application_id: String::new(),
        profiles: vec![creative, deep, repeat],
        rules: Vec::new(),
        automatic: false,
        record_time: false,
        appearance: Appearance::Dark,
        extra: Default::default(),
    }
}

impl Default for Profile {
    fn default() -> Self {
        create_profile("Untitled presence")
    }
}

impl Default for Config {
    fn default() -> Self {
        default_config()
    }
}

impl Default for RuntimeState {
    fn default() -> Self {
        initial_runtime()
    }
}
