//! Demonstrates the same backend running without Tauri, WebView2, or a GUI.
use presence_core::Backend;
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("Usage: cargo run -p presence-core --example headless -- <data-directory>")?;
    let (backend, warnings) = Backend::open(
        &data,
        Arc::new(|state| {
            println!(
                "connection={} mode={} profiles-active={}",
                state.connection,
                state.mode.as_deref().unwrap_or("idle"),
                state.active_profile_id.is_some()
            );
        }),
    )
    .await;
    for warning in warnings {
        eprintln!("{warning}");
    }
    println!(
        "Loaded {} presences. Press Ctrl+C to stop.",
        backend.config().await.profiles.len()
    );
    tokio::signal::ctrl_c().await?;
    backend.shutdown().await?;
    Ok(())
}
