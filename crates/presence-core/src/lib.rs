//! Presence Control's standalone backend. This crate has no GUI or Tauri dependency.
//!
//! [`Backend`] owns scheduling, persistence and serialized async operations. A UI
//! host forwards its runtime snapshots to the UI and calls shutdown before exit.
//! [`Service`] is available for hosts that need to supply their own scheduling.
//! [`storage::Store`] provides the existing JSON persistence format.
//! The Tauri adapter in this workspace is one possible host; a native Rust GUI
//! or a CLI can use the same API directly.
pub mod backend;
pub mod detection;
pub mod engine;
pub mod model;
pub mod rpc;
pub mod service;
pub mod storage;

pub use backend::Backend;
pub use service::Service;
