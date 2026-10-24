# Verification record

Finalized October 24, 2026, on Windows with Rust 1.98.1 and Slint 1.18.1. The application requires Rust 1.92 or later.

## Completed checks

| Command or inspection | Verified result |
| --- | --- |
| `cargo test --workspace` | Passed: 69 tests, consisting of 52 core and 17 desktop tests; no failures. |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed on the final implementation, including native startup sizing and bounded artwork. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo run -p presence-desktop -- --smoke-test` | Passed 52 native behavior checks after the pinned footer, rounded artwork, and popup refinements. |
| `cargo build --release -p presence-desktop` | Passed; portable executable copied to `release/presence-control.exe`. |
| Active Windows dependency-tree inspection | Slint Winit backend and software renderer; no active Tauri, wry, webview, or Skia dependency. |

The completed 52-check smoke report inspected for this record is:

```text
artifacts/native-smoke/567f6107-b1c4-4bec-8d8a-5746bcbd7dd9/smoke-report.json
```

It reports `passed: true`, 52 checks, no error, and an isolated data directory. Artifacts and the machine-local Cargo build-cache configuration are Git ignored.

## Coverage

The core tests exercise configuration compatibility, unknown extension fields, Unicode/template validation, exact rule priority/matching, playback timing, Discord framing/handshake/ping/acknowledgment/error/disconnect behavior through mock IPC, recording accounting, archive identity, failed writes, corruption preservation, import/export, and reopening persisted state. Native detection tests call the actual Windows APIs and compare executable file identity so packaged-app filesystem virtualization is handled correctly.

The desktop tests exercise saved/draft transitions, profile/rule CRUD, ordering, inherited unknown fields, preview templates/timers, activity identity/projections, native instance signaling, malformed/insecure artwork rejection, image dimension bounds before caching, and shutdown retry after a real storage failure. The shutdown regression verifies that the worker remains alive after a failed checkpoint, accepts a second close request, and persists pending archive cleanup after the obstruction is removed.

The smoke run opens the actual Slint window and drives its callbacks through the asynchronous Rust host. It checks profile search, preview updates, save/discard/cancel and save-and-continue, profile/button/playback/artwork edits, duplication/deletion, rule creation/editing/priority/toggles, actual window detection, validation and failed-save feedback, automatic/recording preferences, Stop, activity/archive operations, and configuration/history persistence after reopening the backend. Screenshots cover the main pages, editor tabs, dialogs, validation/error states, and a narrower window.

Smoke checks use generated data beneath `artifacts/native-smoke/<run-id>/data`; ordinary application data and the source Rust rewrite's data are not used.

## Portable build and manual handoff

The portable executable is `release/presence-control.exe` (17,239,040 bytes). Its SHA-256 is:

```text
87efaec75e11425e3e5bb03cb77629ca14c67f9643e5a467a3f6fbb6f26eaf90
```

All final smoke screenshots were reviewed, including the studio, preview, pinned editor footer, narrow layout, settings, activity, archive, and confirmation dialogs. A Windows spot check confirmed that the app renders, maximizes, and accepts an actual keyboard edit. Startup sizing was then adjusted to fit and center the window within the Windows work area.

The user requested that further computer-use testing be handed over to them. No further computer-use testing was performed after that request. The remaining manual checks are listed in [manual-testing.md](manual-testing.md), including startup placement, keyboard/dropdown behavior, real Discord publishing, native file dialogs, and full interactive workflows. The test window uses isolated data under `artifacts/manual-ui/data`; close it before launching the portable app for normal testing.

## Practical limits

Callback smoke checks establish the native host/backend path and persisted behavior. They do not simulate every mouse/keyboard interaction or automatically establish visual fidelity; screenshots require review. Real Discord publishing needs an actual application ID and Discord desktop and has not been verified here. Native import/export dialogs are supplied by the Windows Rust host; their complete manual interaction is outside the callback smoke coverage. Installer and tray behavior are not implemented.
