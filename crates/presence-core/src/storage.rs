use crate::engine::{validate_archive_data, validate_config, validate_usage_data};
use crate::model::{default_config, ArchiveData, Config, UsageData};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Store {
    pub config_path: PathBuf,
    pub usage_path: PathBuf,
    pub archive_path: PathBuf,
}

pub struct Loaded {
    pub store: Store,
    pub config: Config,
    pub usage: UsageData,
    pub archive: ArchiveData,
    pub warnings: Vec<String>,
}

fn load_or_recover<T>(
    path: &mut PathBuf,
    validate: impl FnOnce(&Value) -> Result<T, String>,
    fallback: T,
    warning: &str,
    warnings: &mut Vec<String>,
) -> T {
    match fs::read_to_string(&*path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fallback,
        result => {
            let parsed = result
                .map_err(|e| e.to_string())
                .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()))
                .and_then(|value| validate(&value));
            match parsed {
                Ok(value) => value,
                Err(_) => {
                    warnings.push(warning.to_owned());
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                    *path = path
                        .with_file_name(format!("{stem}-recovery-{}.json", uuid::Uuid::new_v4()));
                    fallback
                }
            }
        }
    }
}

impl Store {
    pub fn load(root: &Path) -> Loaded {
        let mut store = Self {
            config_path: root.join("config.json"),
            usage_path: root.join("usage.json"),
            archive_path: root.join("archive.json"),
        };
        let mut warnings = Vec::new();
        let config = load_or_recover(
            &mut store.config_path,
            validate_config,
            default_config(),
            "Saved configuration could not be loaded. Your original file has been preserved.",
            &mut warnings,
        );
        let usage = load_or_recover(
            &mut store.usage_path,
            validate_usage_data,
            UsageData::default(),
            "Saved application time could not be loaded. The original file has been preserved.",
            &mut warnings,
        );
        let archive = load_or_recover(
            &mut store.archive_path,
            validate_archive_data,
            ArchiveData::default(),
            "The activity archive could not be loaded. The original file has been preserved.",
            &mut warnings,
        );
        Loaded {
            store,
            config,
            usage,
            archive,
            warnings,
        }
    }

    pub fn save_config(&self, config: &Config) -> Result<(), String> {
        atomic_json(&self.config_path, config)
    }
    pub fn save_usage(&self, usage: &UsageData) -> Result<(), String> {
        atomic_json(&self.usage_path, usage)
    }
    pub fn save_archive(&self, archive: &ArchiveData) -> Result<(), String> {
        atomic_json(&self.archive_path, archive)
    }
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid data path.")?;
    fs::create_dir_all(parent).map_err(|e| format!("Could not create the data folder: {e}"))?;
    let temp = parent.join(format!(".presence-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| format!("Could not save local data: {e}"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("Could not save local data: {e}"))?;
        drop(file);
        fs::rename(&temp, path).map_err(|e| format!("Could not replace local data: {e}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Reads at most 1 MiB, including when the file changes while it is read.
pub fn read_config_import(path: &Path) -> Result<Config, String> {
    use std::io::Read;
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("Configuration files must be under 1 MB.".into());
    }
    let value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid configuration JSON: {e}"))?;
    validate_config(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_files_are_preserved_and_recovery_files_receive_writes() {
        let root = tempfile::tempdir().unwrap();
        for name in ["config.json", "usage.json", "archive.json"] {
            fs::write(root.path().join(name), b"original invalid bytes").unwrap();
        }
        let loaded = Store::load(root.path());
        assert_eq!(loaded.warnings.len(), 3);
        loaded.store.save_config(&loaded.config).unwrap();
        loaded.store.save_usage(&loaded.usage).unwrap();
        loaded.store.save_archive(&loaded.archive).unwrap();
        for name in ["config.json", "usage.json", "archive.json"] {
            assert_eq!(
                fs::read(root.path().join(name)).unwrap(),
                b"original invalid bytes"
            );
        }
        assert!(loaded.store.config_path.exists());
    }

    #[test]
    fn atomic_save_replaces_previous_data_and_leaves_no_temporary_files() {
        let root = tempfile::tempdir().unwrap();
        let loaded = Store::load(root.path());
        loaded.store.save_config(&loaded.config).unwrap();
        let mut config = loaded.config;
        config.application_id = "123456789012345678".into();
        loaded.store.save_config(&config).unwrap();
        let restored = Store::load(root.path());
        assert_eq!(restored.config.application_id, config.application_id);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn import_rejects_oversized_or_malformed_files_before_replacing_data() {
        let root = tempfile::tempdir().unwrap();
        let backup = root.path().join("backup.json");
        fs::write(&backup, vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert!(read_config_import(&backup)
            .unwrap_err()
            .contains("under 1 MB"));
        fs::write(&backup, b"invalid json").unwrap();
        assert!(read_config_import(&backup)
            .unwrap_err()
            .contains("Invalid configuration JSON"));
        fs::write(&backup, b"{}").unwrap();
        assert!(read_config_import(&backup).is_err());
    }
}
