# Verification record

Finalized October 24, 2026, on Windows with Rust 1.98.1 and Slint 1.18.1. The application requires Rust 1.92 or later.

## Completed checks

| Command or inspection | Verified result |
| --- | --- |
| `cargo test --workspace` | Passed: 72 tests, consisting of 52 core and 20 desktop tests; no failures. |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed on the final implementation, including native startup sizing and bounded artwork. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo run -p presence-desktop -- --smoke-test` | Passed 60 native behavior checks, including global ID drafts, persistence, inheritance, and failed Go live saves. |
| `cargo build --release -p presence-desktop` | Passed; portable executable copied to `release/presence-control.exe` and SHA-256 verified against the build output. |
| Active Windows dependency-tree inspection | Slint Winit backend and software renderer; no active Tauri, wry, webview, or Skia dependency. |

The completed 60-check smoke report inspected for this record is:

```text
artifacts/native-smoke/586c0a75-a3b3-4692-a71b-b36554424bb2/smoke-report.json
```

It reports `passed: true`, 60 checks, no error, and an isolated data directory. Artifacts and the machine-local Cargo build-cache configuration are Git ignored.

## Coverage

The core tests exercise configuration compatibility, unknown extension fields, Unicode/template validation, exact rule priority/matching, playback timing, Discord framing/handshake/ping/acknowledgment/error/disconnect behavior through mock IPC, recording accounting, archive identity, failed writes, corruption preservation, import/export, and reopening persisted state. Native detection tests call the actual Windows APIs and compare executable file identity so packaged-app filesystem virtualization is handled correctly.

The desktop tests exercise saved/draft transitions, profile/rule CRUD, ordering, inherited unknown fields, preview templates/timers, activity identity/projections, native instance signaling, malformed/insecure artwork rejection, image dimension bounds before caching, and shutdown retry after a real storage failure. The shutdown regression verifies that the worker remains alive after a failed checkpoint, accepts a second close request, and persists pending archive cleanup after the obstruction is removed.

Global application ID regressions verify that Go live includes a pending Settings ID when the profile inherits it, trims surrounding whitespace, saves profile edits in the same candidate, and leaves saved state untouched until persistence succeeds. A profile override remains independent of invalid or pending global edits. Native callback checks verify Settings save/validation, the saved connection label, preserving profile and Settings drafts during unrelated saves, retention after a failed Go live write, successful retry, and reopening the persisted global ID. The failed activation write deliberately prevents any real Discord connection.

The pointer regression dispatches actual move/press/release events through the production Slint controls in a headless software window. It reproduced the first-click failure before the fix: the button callback count remained zero. It now verifies first-click activation of buttons, icons, toggles, dropdowns and their options, text input, sidebar navigation, profile cards, and all editor tabs. It also checks keyboard focus after clicks, Space/Enter activation, arrow-key tab navigation, disabled buttons, and modal background blocking. The focus scopes no longer consume pointer presses; the click handler explicitly focuses and activates the control together.

The smoke run opens the actual Slint window and drives its callbacks through the asynchronous Rust host. It checks profile search, preview updates, save/discard/cancel and save-and-continue, profile/button/playback/artwork edits, duplication/deletion, rule creation/editing/priority/toggles, actual window detection, validation and failed-save feedback, automatic/recording preferences, Stop, activity/archive operations, and configuration/history persistence after reopening the backend. Screenshots cover the main pages, editor tabs, dialogs, validation/error states, and a narrower window.

Smoke checks use generated data beneath `artifacts/native-smoke/<run-id>/data`; ordinary application data and the source Rust rewrite's data are not used.

## Portable build and manual handoff

The portable executable including the global application ID and single-click input fixes is `release/presence-control.exe` (17,306,624 bytes). Its SHA-256 is:

```text
6e6a6477e33bd8ed1778687dd19680e5504550a69521c467bf19ec6b0ffffc79
```

All final smoke screenshots were reviewed, including the studio, preview, pinned editor footer, narrow layout, settings, activity, archive, and confirmation dialogs. A Windows spot check confirmed that the app renders, maximizes, and accepts an actual keyboard edit. Startup sizing was then adjusted to fit and center the window within the Windows work area.

The user requested that further computer-use testing be handed over to them. No further computer-use testing was performed after that request. The remaining manual checks are listed in [manual-testing.md](manual-testing.md), including startup placement, keyboard/dropdown behavior, real Discord publishing, native file dialogs, and full interactive workflows. The test window uses isolated data under `artifacts/manual-ui/data`; close it before launching the portable app for normal testing.

## Practical limits

Callback smoke checks establish the native host/backend path and persisted behavior. They do not simulate every mouse/keyboard interaction or automatically establish visual fidelity; screenshots require review. Real Discord publishing needs an actual application ID and Discord desktop and has not been verified here. Native import/export dialogs are supplied by the Windows Rust host; their complete manual interaction is outside the callback smoke coverage. Installer and tray behavior are not implemented.
