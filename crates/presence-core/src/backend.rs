//! A reusable desktop/CLI host. Scheduling and state management do not depend on a GUI.
use crate::{
    engine,
    model::{Config, DetectedApp, RuntimeState},
    service::{OnChange, Service},
    storage::{self, Store},
};
use serde_json::Value;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
};
use tokio::sync::Mutex;

struct Inner {
    service: Mutex<Service>,
    store: Store,
    quitting: AtomicBool,
}

/// Cloneable frontend-neutral handle. All mutations are serialized.
#[derive(Clone)]
pub struct Backend {
    inner: Arc<Inner>,
}

impl Backend {
    /// Opens the existing JSON workspace and starts automatic checks. Call from a
    /// Tokio runtime; callback invocations can arrive on any runtime worker.
    /// Invalid local files are preserved and warnings are also emitted as logs.
    pub async fn open(root: &Path, on_change: OnChange) -> (Self, Vec<String>) {
        let loaded = Store::load(root);
        let mut service = Service::new(loaded.config, loaded.usage, loaded.archive);
        let usage_store = loaded.store.clone();
        let archive_store = loaded.store.clone();
        service.set_persistence(
            Arc::new(move |usage| usage_store.save_usage(usage)),
            Arc::new(move |archive| archive_store.save_archive(archive)),
        );
        service.set_on_change(on_change);
        for warning in &loaded.warnings {
            service.log("error", warning);
        }
        let mut disconnects = service.subscribe_disconnects();
        let backend = Self {
            inner: Arc::new(Inner {
                service: Mutex::new(service),
                store: loaded.store,
                quitting: AtomicBool::new(false),
            }),
        };
        let weak = Arc::downgrade(&backend.inner);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(inner) = weak.upgrade() else { break };
                if inner.quitting.load(Ordering::SeqCst) {
                    break;
                }
                let mut service = inner.service.lock().await;
                if !inner.quitting.load(Ordering::SeqCst) {
                    service.tick().await;
                }
            }
        });
        let weak = Arc::downgrade(&backend.inner);
        tokio::spawn(async move {
            loop {
                match disconnects.recv().await {
                    Ok(event) => {
                        let Some(inner) = weak.upgrade() else { break };
                        if inner.quitting.load(Ordering::SeqCst) {
                            break;
                        }
                        let mut service = inner.service.lock().await;
                        if !inner.quitting.load(Ordering::SeqCst)
                            && event.connection_id == service.connection_id()
                            && !service.rpc_is_connected()
                        {
                            service.on_disconnect(event.at);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
        (backend, loaded.warnings)
    }

    pub async fn config(&self) -> Config {
        self.inner.service.lock().await.config.clone()
    }
    pub async fn runtime(&self) -> RuntimeState {
        self.inner.service.lock().await.runtime()
    }

    async fn running_service(&self) -> Result<tokio::sync::MutexGuard<'_, Service>, String> {
        let service = self.inner.service.lock().await;
        if self.inner.quitting.load(Ordering::SeqCst) {
            return Err("The backend has shut down.".into());
        }
        Ok(service)
    }

    pub async fn save_config(&self, input: &Value) -> Result<Config, String> {
        let next = engine::validate_config(input)?;
        let mut service = self.running_service().await?;
        self.inner.store.save_config(&next)?;
        if let Err(error) = service.set_config(next.clone()).await {
            if service.state.error.is_none() {
                service.log("error", &error);
            }
        }
        drop(service);
        Self::schedule_tick(Arc::downgrade(&self.inner));
        Ok(next)
    }

    fn schedule_tick(weak: Weak<Inner>) {
        tokio::spawn(async move {
            if let Some(inner) = weak.upgrade() {
                let mut service = inner.service.lock().await;
                if !inner.quitting.load(Ordering::SeqCst) {
                    service.tick().await;
                }
            }
        });
    }

    pub async fn activate(&self, id: &str) -> Result<(), String> {
        self.running_service().await?.activate(id).await
    }

    pub async fn stop(&self) -> Result<(), String> {
        let mut service = self.running_service().await?;
        let mut next = service.config.clone();
        next.automatic = false;
        let saved = self.inner.store.save_config(&next);
        service.config = next;
        // Stopping presence must still work when the data folder is unwritable.
        // Return a save failure only after automatic retries and RPC are stopped.
        let stopped = service.stop().await;
        saved.and(stopped)
    }

    pub async fn scan(&self) -> Result<Vec<DetectedApp>, String> {
        self.running_service().await?.scan().await
    }

    pub async fn archive_activity(&self, kind: &str, id: &str) -> Result<RuntimeState, String> {
        self.running_service()
            .await?
            .archive_activity(kind, id)
            .await
    }
    pub async fn delete_activity(&self, kind: &str, id: &str) -> Result<RuntimeState, String> {
        self.running_service()
            .await?
            .delete_activity(kind, id)
            .await
    }
    pub async fn delete_archived_activity(
        &self,
        kind: &str,
        id: &str,
    ) -> Result<RuntimeState, String> {
        self.running_service()
            .await?
            .delete_archived_activity(kind, id)
            .await
    }

    /// Native UI, Tauri, and CLI hosts all use the same bounded import path.
    pub async fn import_from(&self, path: &Path) -> Result<Config, String> {
        let mut next = storage::read_config_import(path)?;
        next.automatic = false;
        let mut service = self.running_service().await?;
        service.stop().await?;
        self.inner.store.save_config(&next)?;
        service.set_config(next.clone()).await?;
        service.log("info", "Configuration imported. Automatic mode is off.");
        Ok(next)
    }

    pub async fn export_to(&self, path: &Path) -> Result<(), String> {
        storage::atomic_json(path, &self.config().await)
    }

    /// Call before the host exits to checkpoint usage and disconnect Discord.
    pub async fn shutdown(&self) -> Result<(), String> {
        self.inner.quitting.store(true, Ordering::SeqCst);
        let mut service = self.inner.service.lock().await;
        service.dispose().await;
        service.flush_usage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn standalone_host_persists_imports_exports_and_restarts_without_tauri() {
        let root = tempfile::tempdir().unwrap();
        let (backend, warnings) = Backend::open(root.path(), Arc::new(|_| {})).await;
        assert!(warnings.is_empty());
        let mut config = backend.config().await;
        config.profiles[0].name = "Native Rust host".into();
        config.application_id = "123456789012345678".into();
        backend
            .save_config(&serde_json::to_value(config).unwrap())
            .await
            .unwrap();
        let backup = root.path().join("backup.json");
        backend.export_to(&backup).await.unwrap();
        let mut imported: Value = serde_json::from_slice(&std::fs::read(&backup).unwrap()).unwrap();
        imported["automatic"] = Value::Bool(true);
        std::fs::write(&backup, serde_json::to_vec(&imported).unwrap()).unwrap();
        let restored = backend.import_from(&backup).await.unwrap();
        assert!(!restored.automatic);
        assert_eq!(restored.profiles[0].name, "Native Rust host");
        assert!(backend
            .runtime()
            .await
            .logs
            .iter()
            .any(|log| log.message.contains("Configuration imported")));
        backend.shutdown().await.unwrap();
        drop(backend);
        let (restarted, _) = Backend::open(root.path(), Arc::new(|_| {})).await;
        assert_eq!(restarted.config().await, restored);
        restarted.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_rejects_mutations_from_another_cloned_frontend_handle() {
        let root = tempfile::tempdir().unwrap();
        let (backend, _) = Backend::open(root.path(), Arc::new(|_| {})).await;
        let other_ui = backend.clone();
        let config = serde_json::to_value(backend.config().await).unwrap();
        backend.shutdown().await.unwrap();
        assert!(other_ui
            .save_config(&config)
            .await
            .unwrap_err()
            .contains("shut down"));
        assert!(other_ui
            .activate("creative-flow")
            .await
            .unwrap_err()
            .contains("shut down"));
        assert!(other_ui.stop().await.unwrap_err().contains("shut down"));
        assert!(other_ui.scan().await.unwrap_err().contains("shut down"));
        assert!(!root.path().join("config.json").exists());
    }

    #[tokio::test]
    async fn stop_still_clears_presence_when_disabling_automatic_cannot_be_saved() {
        let root = tempfile::tempdir().unwrap();
        let (backend, _) = Backend::open(root.path(), Arc::new(|_| {})).await;
        let mut config = backend.config().await;
        config.automatic = true;
        backend
            .save_config(&serde_json::to_value(&config).unwrap())
            .await
            .unwrap();
        {
            let mut service = backend.inner.service.lock().await;
            // A manual activity with automatic mode enabled must remain stopped,
            // including when it is already disconnected from Discord.
            service.state.mode = Some("manual".into());
            service.state.active_profile_id = Some(config.profiles[0].id.clone());
        }
        let config_path = root.path().join("config.json");
        std::fs::remove_file(&config_path).unwrap();
        std::fs::create_dir(&config_path).unwrap();

        assert!(backend.stop().await.unwrap_err().contains("local data"));
        assert!(!backend.config().await.automatic);
        let runtime = backend.runtime().await;
        assert_eq!(runtime.connection, "disconnected");
        assert!(runtime.active_profile_id.is_none());
        assert!(runtime.mode.is_none());

        std::fs::remove_dir(&config_path).unwrap();
        backend.stop().await.unwrap();
        assert!(!Store::load(root.path()).config.automatic);
        backend.shutdown().await.unwrap();
    }
}
