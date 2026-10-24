# Rust rewrite inheritance and native UI migration

## Provenance

The source is the sibling repository **Presence Control Rust Rewrite**, at commit:

```text
f91dd6b8c46ba00017bdd7d519eb1a757bded7b7
```

Its working tree was clean when inspected on October 23, 2026. The new repository was initialized independently; source Git history, frontend package dependencies, build output, Tauri host, React/TypeScript files, and source application data were not copied into the new application.

The inherited material is `crates/presence-core` and `assets` from the Rust rewrite. Its `src/App.tsx`, `src/styles.css`, shared renderer contracts, and saved screenshots supplied the behavior/layout reference. No Electron implementation was used as the migration source.

## Native architecture

The backend's serializable configuration and runtime contract remains the boundary between the service and UI. `Backend` owns the Tokio scheduling tasks, serialized service access, Discord IPC, usage accounting, persistence, imports, exports, and lifecycle. The Slint adapter queues runtime changes onto the native event loop; backend work does not run in a webview.

Rust replaces the former renderer's profile drafts, CRUD operations, template preview, validation, playback display, application-rule edits, and activity row formatting. Slint declarative markup is compiled at build time. There is no browser bridge or frontend development server.

The source detection module still invoked `powershell.exe` to execute `Get-Process`. This module was replaced with `windows-sys` calls: `EnumWindows`, `GetWindowTextW`, `GetWindowThreadProcessId`, `OpenProcess`, `QueryFullProcessImageNameW`, and version-resource queries. A Tokio blocking worker performs enumeration and metadata reads. Only visible, ownerless windows with titles are considered; minimized main windows remain eligible. Multiple windows from the same PID produce one application row. Protected or exited processes are skipped. Titles and metadata retain Unicode. Results keep the source's case-insensitive process ordering and filters for self, `electron.exe`, and the legacy `Presence Control.exe` executable.

The product-name lookup checks the executable's declared version-resource language/codepage pairs, then standard English string tables, and falls back to the executable filename without its extension. The Windows APIs are called directly from Rust and no child script process is launched.

## Behavior inventory

| Area | Preserved contract or native replacement |
| --- | --- |
| Configuration | Compatible JSON version, IDs, legacy-field normalization, unknown extension fields |
| Profile editing | Saved/draft distinction; explicit save; selection/new/duplicate/delete; profile references in rules |
| Activity payload | Four activity types; display name/details/state; artwork keys or URLs and hover text; up to two buttons |
| Preview | Resolved template text, theme artwork, elapsed time and optional media progress |
| Connection | Global application ID, per-profile override, Developer Portal action, runtime error feedback |
| Automatic mode | First enabled matching rule; case-insensitive exact process name; optional title filter; 15-second checks |
| Manual mode | Manual takes priority; Stop clears presence and disables automatic mode |
| Rules | Create/edit/delete/enable/disable; priority changes; discovered process selection |
| Recording | Opt-in; starts after confirmed automatic publish; disconnect/manual/disabled/changed-app pauses |
| Activity | Totals, recent events, latest 500 sessions; archive/delete operations with active-recording guards |
| Archive | Permanent events, sessions, and application totals; deletion persisted with usage cleanup |
| Persistence | Explicit saves, atomic replacement, corrupt source-file preservation, recovery writes |
| Import/export | Native JSON file dialogs; import validates before replacement and disables automatic mode |
| Shutdown | Disconnect and checkpoint recorded time before runtime teardown |
| Instance management | Windows native mutex and existing-window activation, scoped to the data directory |

The Slint layout follows the Rust rewrite's black surfaces, sidebar, typography, separators, editor tabs, white primary action, saved-presence cards, and preview arrangement. Source screenshots guided adjustments to the native layout, while native controls, text metrics, window chrome, and rule-order controls retain some platform differences.

## Verification responsibilities

Inherited core tests cover configuration and data validation, Unicode/template bounds, rule matching, playback timing, IPC framing and acknowledgments, connect/disconnect failures, usage accounting, archiving, failed writes, corruption recovery, import/export, and restart behavior.

Native detection tests cover filtering, PID deduplication, Unicode metadata, case-preserving sort behavior, querying the current executable, and a real Windows window/process scan. Desktop editor tests cover draft transitions, profile/rule updates, preview behavior, and activity projections. Tests using mock IPC verify protocol behavior without publishing to a real Discord account.

On October 23, 2026, `cargo test --workspace` passed all 69 tests: 52 core tests and 17 desktop tests. This includes the actual Windows scan, executable file-identity query, mock Discord IPC, service accounting, persistence/restart, native instance signaling, the worker's failed-checkpoint/shutdown retry, and artwork dimension bounds before caching. Comparing executable file identities also accepts Windows packaged-app virtualized paths that refer to the same underlying file.

The final native smoke run passed 52 checks through the actual Slint callbacks and asynchronous host after the pinned editor footer, rounded SVG artwork, and popup refinements. Its scope includes save-and-continue from the unsaved-changes dialog, Windows application scanning, profile/rule changes, failure feedback, archive management, and reopening persisted data. Final workspace Clippy, formatting, and the optimized release build also passed. See [verification.md](verification.md) for the validation record and [manual-testing.md](manual-testing.md) for the user's remaining interactive checks.

The active Windows dependency tree uses Slint's Winit backend and software renderer. It contains no Tauri, wry, webview, or Skia runtime dependency. Optional packages can still appear in Cargo.lock without being activated for this build.

A successful build, native callback smoke run, or mock transport test does not verify real Discord publishing, installer behavior, or every manual native-dialog interaction.
