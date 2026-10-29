# Verification record

Finalized October 24, 2026, on Windows with Rust 1.98.1 and Slint 1.18.1. The application requires Rust 1.92 or later.

## Edge-case audit — October 29, 2026

Reviewed the complete Rust/Slint rewrite's storage, recording and archive lifecycle, Discord IPC, validation, Windows detection, editor, asynchronous worker, artwork loading, and native host.

Fixed confirmed failure paths:

- Saved recovery workspaces were abandoned on restart while their original files remained corrupt. Startup now resumes the newest valid recovery file, with deterministic ordering and preference for a valid original. Corrupt files are preserved.
- A failed configuration write prevented Stop from disabling automatic mode or clearing presence. Stop now completes the runtime action, reports the persistence error, and updates the UI configuration even on failure. A retry saves the disabled setting.
- Failed application-rule saves discarded the dialog and draft. Both now remain available for retry until persistence succeeds.
- Profile inputs accepted visible edits while backend callbacks ignored them during busy operations. Profile and rule controls now pause consistently during those operations.
- Imports accepted an empty rule ID that could produce usage history rejected on restart. Empty IDs are rejected before saving.
- Template expansion could split an emoji at Discord's 128 UTF-16-unit boundary or publish an empty custom name. Output now preserves complete characters and omits empty resolved names; truncation also avoids an intermediate UTF-16 buffer.

Activity refresh now builds current history once and partitions it into events, totals, and sessions. This removes two redundant projections and their archive scans per refresh while preserving order, live timing, and separate archives.

Verification passed on the final source:

- `cargo test --workspace --locked`: **97 tests passed** (63 core, 34 desktop), including eight new tests and expanded validation/UI coverage.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo fmt --all -- --check`, and `git diff --check`: passed.
- Native workflow smoke: **77 checks passed**, using isolated data. Report: `artifacts/native-smoke/7fadde45-1e73-4a3a-ae79-605f12cc1978/smoke-report.json`.

These are source changes; the existing 0.1.2 portable executable and installer were not rebuilt. Real Discord publishing was not exercised during this audit.

## Windows installer 0.1.2 — October 28, 2026

The appearance release is packaged as `release/Presence-Control-0.1.2-windows-x64-setup.exe` using NSIS 3.13. Both workspace crates, the portable executable, and the installer use version **0.1.2**. Settings and the workspace footer now display the Cargo package version. This release includes the native repaint backport and Dark/Light/System appearance settings.

Verification passed: **89 Rust tests**, Clippy with `-D warnings`, formatting, **28 isolated installer lifecycle checks**, and **77 installed native workflow checks**. The installer report is `artifacts/installer-test/38b5e34c-7a04-4468-b1ec-254508e18b14/installer-report.json`; the installed workflow report is `artifacts/native-smoke/58db485d-eb93-4759-a424-0c1dbd5e1398/smoke-report.json`. Installation, locked-file refusal, reinstallation, bundled runtime loading, shortcuts, registration, and uninstall passed. Normal application data and additional installation-directory files were preserved.

Setup is **6,501,985 bytes**, SHA-256 `d1b0f61909ac26e9ad83ca0df420b2713b556a8e2d12bec4b760006fe2a723f2`. The portable executable is **17,632,256 bytes**, SHA-256 `7e320e7218ea0a9fb748d24f18a073554d14110a0e7086568e141eff61ba0211`. All 15 entries in `release/SHA256SUMS.txt` were verified.

## Appearance update — October 28, 2026

- `cargo test --workspace`: **89 passed** (58 core, 31 desktop). New checks cover legacy OLED dark defaults, appearance validation and workspace round trips, live system-theme changes, explicit overrides, preservation of unsaved drafts, and pointer/keyboard interaction with the mode selector.
- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check`: passed.
- Native smoke: **77 checks passed** for both the debug build and the updated portable release. The release report is `artifacts/native-smoke/ade12b8c-0455-4849-8e5f-24f61e7e0e4a/smoke-report.json`. This includes saving Light/System/Dark, reloading the preference through a fresh backend, and restoring OLED dark.
- Reviewed native light-theme screenshots of Settings, the studio, application rules, and the 800×620 Settings layout, plus the OLED dark Settings screen. Additional captures cover artwork, integrations, activity, and archives.
- `cargo build --release -p presence-desktop`: passed. `release/presence-control.exe` matches the compiled release, and its entry in `release/SHA256SUMS.txt` was updated. SHA-256: `8ddd21ca5b64d7488234620744eb5f35dd45d26c319493d365858ec48de053b3`.

## Completed checks

| Command or inspection | Verified result |
| --- | --- |
| `cargo test --workspace` | Passed: 77 tests, consisting of 52 core and 25 desktop tests; no failures. |
| `cargo test -p i-slint-renderer-software --lib` | Passed: 10 renderer tests, including coverage correction for all foreground luminances. |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed on the final implementation, including native startup sizing and bounded artwork. |
| `cargo fmt --all -- --check` | Passed. |
| Native smoke at 100%, 125%, and 150% scaling | Passed all 63 behavior checks at each scale; captures confirm the physical DPI changes. |
| `cargo build --release -p presence-desktop` | Passed; portable executable copied to `release/presence-control.exe` and SHA-256 verified against the build output. |
| Active Windows dependency-tree inspection | Slint Winit backend and software renderer; no active Tauri, wry, webview, or Skia dependency. |

The completed 63-check smoke report inspected for this record is:

```text
artifacts/native-smoke/b636059a-df33-4e8b-8c84-d2b58c8ecf33/smoke-report.json
```

It reports `passed: true`, 63 checks, no error, and an isolated data directory. Artifacts and the machine-local Cargo build-cache configuration are Git ignored.

## Coverage

The core tests exercise configuration compatibility, unknown extension fields, Unicode/template validation, exact rule priority/matching, playback timing, Discord framing/handshake/ping/acknowledgment/error/disconnect behavior through mock IPC, recording accounting, archive identity, failed writes, corruption preservation, import/export, and reopening persisted state. Native detection tests call the actual Windows APIs and compare executable file identity so packaged-app filesystem virtualization is handled correctly.

The desktop tests exercise saved/draft transitions, profile/rule CRUD, ordering, inherited unknown fields, preview templates/timers, activity identity/projections, native instance signaling, malformed/insecure artwork rejection, image dimension bounds before caching, and shutdown retry after a real storage failure. The shutdown regression verifies that the worker remains alive after a failed checkpoint, accepts a second close request, and persists pending archive cleanup after the obstruction is removed.

Global application ID regressions verify that Go live includes a pending Settings ID when the profile inherits it, trims surrounding whitespace, saves profile edits in the same candidate, and leaves saved state untouched until persistence succeeds. A profile override remains independent of invalid or pending global edits. Native callback checks verify Settings save/validation, the saved connection label, preserving profile and Settings drafts during unrelated saves, retention after a failed Go live write, successful retry, and reopening the persisted global ID. The failed activation write deliberately prevents any real Discord connection.

The pointer regression dispatches actual move/press/release events through the production Slint controls in a headless software window. It reproduced the first-click failure before the fix: the button callback count remained zero. It now verifies first-click activation of buttons, icons, toggles, dropdowns and their options, text input, sidebar navigation, profile cards, and all editor tabs. It also checks keyboard focus after clicks, Space/Enter activation, arrow-key tab navigation, disabled buttons, and modal background blocking. The focus scopes no longer consume pointer presses; the click handler explicitly focuses and activates the control together.

The smoke run opens the actual Slint window and drives its callbacks through the asynchronous Rust host. It checks profile search, preview updates, save/discard/cancel and save-and-continue, profile/button/playback/artwork edits, duplication/deletion, rule creation/editing/priority/toggles, actual window detection, validation and failed-save feedback, automatic/recording preferences, Stop, activity/archive operations, and configuration/history persistence after reopening the backend. Screenshots cover the main pages, editor tabs, dialogs, validation/error states, and a narrower window.

Smoke checks use generated data beneath `artifacts/native-smoke/<run-id>/data`; ordinary application data and the source Rust rewrite's data are not used.

## Portable build and manual handoff

The portable executable including the global application ID, single-click input, typography, rounded artwork, saved-card icons, and timed notifications is `release/presence-control.exe`. Its current SHA-256 is recorded in `release/SHA256SUMS.txt` and checked against the Cargo release build.

All final smoke screenshots were reviewed, including the studio, preview, pinned editor footer, narrow layout, settings, activity, archive, and confirmation dialogs. A Windows spot check confirmed that the app renders, maximizes, and accepts an actual keyboard edit. Startup sizing was then adjusted to fit and center the window within the Windows work area.

The user requested that further computer-use testing be handed over to them. No further computer-use testing was performed after that request. The remaining manual checks are listed in [manual-testing.md](manual-testing.md), including startup placement, keyboard/dropdown behavior, real Discord publishing, native file dialogs, and full interactive workflows. The test window uses isolated data under `artifacts/manual-ui/data`; close it before launching the portable app for normal testing.

## Row dividers and program-wide text rendering — October 24, 2026

Application-rule and detected-window separators explicitly sit at the top of each row. Slint previously centered these unpositioned children vertically, drawing the line through the row's icon and text. Other dividers already have an explicit position or belong to a layout.

The window uses `Segoe UI Variable Text`, the first family in the preceding Rust/web studio's stylesheet. Windows resolves it to `SegUIVar.ttf`, with variable weights from 300 to 700. Shared typography roles retain the preceding stylesheet's weights: body 400, labels/navigation 500, section titles/buttons 600, page headings 550, and wordmark 650. The earlier weight increases have been removed; sharpness is now handled by the shared text renderer.

The pinned Slint software-renderer patch enables Windows glyph hinting and applies luminance-dependent gamma/contrast correction to glyph masks before item opacity. This covers headings, labels, buttons, tabs, `TextInput`, helper text, and standard controls on every page. RGB/RGBA artwork and other primitives keep their existing blending. The [patch notes](../vendor/i-slint-renderer-software/LOCAL_CHANGES.md) document its scope, references, licensing, and upgrade process. The application remains native Rust without a browser or Skia runtime.

A controlled rendering sample compared the same strings, font families, sizes, colors, and weights before and after the renderer patch. At weight 400, measured grayscale glyph coverage increased from 144,661 to 171,763 in the same sample region (about 19%); all seven weights remain distinct. The two diagnostic images are `artifacts/ui-fix/font-weights-before-raster-fix.png` and `artifacts/ui-fix/font-weights.png`.

The workspace's 72 tests and the renderer's 10 tests pass. Renderer tests cover all 256 foreground luminances, transparent/solid endpoints, monotonic mask coverage, light/dark text, and preservation of item opacity. Native smoke passed all 60 checks at each tested DPI:

| Scale | Studio capture size | Smoke report beneath `artifacts/native-smoke/` |
| --- | --- | --- |
| 100% | 1470 × 1000 | `12853940-08c0-4681-ac3a-36637a203c1a/smoke-report.json` |
| 125% | 1838 × 1250 | `28def0b9-bcb4-4983-b236-c1cc87efef0e/smoke-report.json` |
| 150% | 2205 × 1500 | `187b111b-38ef-4800-a31d-5bb40d55a6a0/smoke-report.json` |

Studio, integrations, Settings, Activity, and Archive captures were reviewed, along with scaled studio/dialog captures and the controlled font sample. The divider fix remains intact. Workspace and renderer Clippy checks with `-D warnings`, workspace formatting, and `git diff --check` passed. The release build passed, and the portable copy's SHA-256 matched the build output. Running that portable executable with `--smoke-test` also passed all 60 checks; its report is `artifacts/native-smoke/c192e8c0-d7e0-4ba0-8082-9f2d34bc72b3/smoke-report.json`.

The updated portable copy is `release/presence-control.exe`. Its predecessor is preserved as `release/presence-control-before-text-rendering-20261024-111114.exe`. `release/THIRD-PARTY-NOTICES.txt` accompanies the build. Relaunch the application to load the new rendering code.

## Rounded artwork, saved-card icons, and timed notifications

The software renderer does not implement rounded clipping, and its gradient fill can paint outside a Rectangle's rounded border. This is consistent with Slint's [documented rounded-clip limitation](https://docs.slint.dev/latest/docs/slint/guide/backends-and-renderers/backends_and_renderers/). HTTPS images now use center-cropped, antialiased RGBA masks made on the artwork worker: 42 px cards with 10 px corners, 88 px editor images with 13 px corners, 62 px previews with 9 px corners, and circular 19 px badges, each rasterized at 3x resolution. Transparent pixels reveal the surrounding UI instead of fallback artwork. Banner gradients and decorative lines use clipped SVGs; selected cards use a rounded solid background.

Saved presence rows now carry their own artwork. Card and preview requests share one URL cache, with four concurrent requests, cancellation for unused URLs, and retry backoff. The cache retains only URLs needed by the filtered saved profiles or current draft, bounded by the workspace's 100-profile limit. Unsaved artwork changes update the editor/preview; saved cards keep their saved image until the profile is saved.

All status notifications use a five-second single-shot timer. Publishing a new message restarts the timer, including an identical message. Manual dismissal stops it. Confirmation dialogs retain their existing interaction. Tests cover source transparency, non-square/thin images, rounded alpha, circular badges, images on unselected saved cards, shared requests, failed URL fallback/backoff, draft separation, expiry, timer replacement, and immediate manual dismissal. Timing tests use a controlled platform clock; native smoke also waits for an actual notification to expire.

The latest smoke runs include `slint-rounded-artwork.png` for review:

| Scale | Report beneath `artifacts/native-smoke/` |
| --- | --- |
| 100% | `0ba7e8b3-a23c-4fd7-962e-37d5dfe19a9b/smoke-report.json` |
| 125% | `07551043-0609-4ffe-b3ac-f431af116df4/smoke-report.json` |
| 150% | `59ad1321-a2d3-4800-a2f5-a483e7df465c/smoke-report.json` |

The final portable executable also passed all 63 native checks in `artifacts/native-smoke/ed01d75f-768c-4699-bb83-52fd5b202fec/smoke-report.json`. It is 17,415,680 bytes, with SHA-256 `851ed47c7d51e11b52794ab35d389fcb692c342e04cb6a24d20c42134cc80c98`, matching the Cargo release output. The previous portable executable is preserved as `release/presence-control-before-artwork-notifications-20261024.exe`.

## Application empty states and drag-and-drop priority

Applications and Open applications use a shared padded empty-state row, with a divider, icon, title, and wrapping helper text. Native captures include the normal 1470 px window and a 900 × 800 px window; the smoke runner waits for the native resize before capturing the narrow layout.

The arrow buttons have been replaced with six-dot priority handles using Slint's built-in [DragArea](https://docs.slint.dev/latest/docs/slint/reference/drag-and-drop/dragarea/) and DropArea. Accepted targets highlight the row and show an insertion line above or below it; the source dims while dragging. Drops save through the existing asynchronous configuration path, retain rule IDs/settings, and refresh automatic priority just like keyboard moves. Same-position drops do not write configuration. Dropping outside a rule cancels; only internal rule payloads are accepted. Dragging is disabled while saving or showing a modal. Focused handles support Up/Down keys.

The new editor regression covers moves across multiple positions, before/after placement, no-ops, invalid IDs, preservation of complete rule data, and leaving saved configuration unchanged until persistence succeeds. A production-window pointer regression covers dragging first-to-last and last-to-first, clicking without dragging, same-row/outside drops, dragging text instead of the handle, keyboard control, Edit/toggle clicks, disabled/modal behavior, and rejection of external plain-text payloads. Native smoke now performs real pointer drags in both directions and checks persisted priority, then reopens the backend to check the complete workspace.

The 100% native report is `artifacts/native-smoke/b636059a-df33-4e8b-8c84-d2b58c8ecf33/smoke-report.json` (all 63 checks passed). Its `slint-integrations-empty.png`, `slint-integrations-empty-narrow.png`, and `slint-integrations-drag.png` were reviewed.

The 150% run also passed all 63 checks in `artifacts/native-smoke/57e25f27-9c7c-4490-b0da-703fa46ce363/smoke-report.json`, including real pointer reorders. Its forced 900 × 800 physical-pixel empty-state capture was also reviewed for spacing and text containment. Workspace tests, Clippy with `-D warnings`, formatting, and diff whitespace checks passed.

The updated portable executable passed all 63 checks in `artifacts/native-smoke/b4cff132-aeb0-4908-9c43-2f2f5603ae62/smoke-report.json`. It is 17,469,440 bytes; SHA-256 `783245c5ea39e7d5a5debd9740d1bdeaed7c55861ccd7c4f684a3adb5b4e7592` matches the Cargo release output and `release/SHA256SUMS.txt`. The previous portable executable is preserved as `release/presence-control-before-integrations-polish-20261024.exe`.

## Activity log alignment and recording placement

Activity section headers now give their description the flexible space and keep the entry count at the right. Every activity row uses a flexible name column, a fixed 112 px right-aligned monospace time column, and a fixed 124 px action group. Totals, events, completed sessions, and archived entries retain the same time alignment despite different name lengths or the archive's missing Archive action.

The active session is excluded from Previous sessions while the backend continues checkpointing it for recovery. Application time displays the live cumulative duration and the current session's elapsed duration, updated by the UI clock between backend checks. When recording ends, the existing session appears in history with its final duration and enabled management controls. Stopped totals and completed durations remain fixed. All projections within a refresh use one clock sample.

`cargo test --workspace` passed all 78 tests. The host/UI regression advances time without a backend update, applies a persisted checkpoint without double counting, and finishes the recording while retaining older history. Clippy with `-D warnings`, formatting, diff whitespace checks, and the release build passed.

The release executable passed all 68 native smoke checks in `artifacts/native-smoke/1072572b-c0a6-49e9-9843-f70c3b731c50/smoke-report.json`. Its additional recording checks use synthetic host snapshots without modifying backend usage or ordinary application data. `slint-activity-recording.png`, `slint-activity-finished.png`, and `slint-archive.png` were reviewed for alignment and placement.

The updated portable executable is `release/presence-control.exe`, 17,487,360 bytes. Its SHA-256 is `fe132eb2d436a0a8e4756a2043cfb0e46bc607c07effde332d849f97350e4e9e`, matching the tested Cargo release output and `release/SHA256SUMS.txt`. The previous copy is preserved as `release/presence-control-before-activity-log-20261024-121655.exe`. Relaunch the portable application to load these changes.

## Application archives follow cumulative totals

Archiving Application time now keeps the total and its row, including during active recording. A `tracksApplicationTime` archive marker binds one saved entry to that total's existing generation ID. Recording checkpoints continue to update Application time; the linked archive updates when recording finishes, pauses, switches applications, disconnects, stops, or shuts down. Repeating Archive creates no duplicate. The row shows In Archive, and its Archive action is disabled while linked. Individual archived sessions remain fixed records.

Delete remains the action that removes an application total. It requires active recording to pause, retains the final archive value, and detaches the saved entry. A future recording creates a new generation and leaves the older archive fixed. Deleting only the archive entry preserves Application time and stops its archive updates. Older application snapshots remain unchanged, including their interrupted-move recovery behavior.

Usage checkpoints reach disk before linked archive updates. Failed archive updates remain pending and retry; restart repairs a stale archive from its matching saved total. Startup detaches orphaned links without recreating deleted totals. Archive mutations cannot accidentally save a pending total update ahead of usage. Shutdown reports an unsaved archive and retains the worker for retry.

All 85 workspace tests passed, including multiple sessions, live archiving, duplicate prevention, app switching, recording completion paths, generation identity, independent deletion, legacy snapshots, interrupted writes, and actual worker/store shutdown failure and retry. Clippy with `-D warnings`, formatting, whitespace checks, and the release build passed.

The tested release passed all 72 native checks in `artifacts/native-smoke/61ad5778-f457-4b89-ad94-c056fe0b87de/smoke-report.json`. The workflow checks the retained Application time row, linked archive metadata, restart persistence, explicit total deletion, retained detached archive, and no resurrection after restart. `slint-application-time-linked.png` and `slint-archive-linked-total.png` were visually reviewed.

The portable copy is `release/presence-control.exe`, 17,502,208 bytes, with SHA-256 `eed1c7ce0bbdf9118c0420d63e47d9bec121a7b2053c6388607d502bae04b01a`, matching the tested release output and `release/SHA256SUMS.txt`. Its predecessor is preserved as `release/presence-control-before-linked-archive-20261024-124151.exe`. Relaunch the application to use this behavior.

## Windows installer 0.1.0 — October 24, 2026

`scripts/build-installer.ps1` builds the native application with Cargo's locked dependency tree and packages it with NSIS. The Windows x64 installer is `release/Presence-Control-0.1.0-windows-x64-setup.exe`. Its version comes from `presence-desktop/Cargo.toml`, and `release/SHA256SUMS.txt` covers setup, the executable, bundled runtime DLLs, and accompanying documentation. This build is unsigned.

The executable's PE import table requires `VCRUNTIME140.dll` and Windows system libraries. Setup includes the x64 Microsoft VC runtime beside the executable, with no runtime download, elevation, or separate prerequisite installation. Module inspection during the installed smoke run confirmed that the bundled `vcruntime140.dll` was loaded.

The installer lifecycle report is `artifacts/installer-test/6464badb-5548-4da3-b177-a7a7e2054271/installer-report.json`: **28 checks passed**. Coverage includes a destination containing spaces and Unicode, binary/runtime hashes, Windows Installed apps registration, correctly quoted uninstall commands, the Start menu shortcut, default optional-desktop behavior, locked-file refusal for both upgrade and uninstall, reusing a registered custom directory on reinstall, and full removal of the owned payload and registry entries. Additional files in the installation directory and normal application data were preserved. Uninstall deletes explicit payload filenames and uses only a nonrecursive directory removal.

The installed executable also completed all **72 native workflow checks** using an isolated smoke workspace. The unchanged application passed **85 workspace tests**. The release executable remains identical to the previously tested Rust build. No computer-use automation was used to operate wizard pages. Interactive wizard layout, the optional desktop shortcut, and a clean second PC check remain in [manual-testing.md](manual-testing.md).

## Version 0.1.1 Windows repaint correction — October 24, 2026

The installed and portable 0.1.0 executables had identical SHA-256 hashes. The reported see-through/stale client areas were investigated as a Windows repaint defect shared by those binaries.

The 0.1.1 build vendors `i-slint-backend-winit` 1.18.1 and backports [Slint PR #13568](https://github.com/slint-ui/slint/pull/13568). Only `winitwindowadapter.rs` differs from the published crate's implementation. Before drawing, the Windows backend reads `GetUpdateRect` while winit still preserves it, converts the physical rectangle to logical coordinates, and marks it dirty. OS-invalidated pixels are repainted even when the application's state has not changed. Ordinary internal redraw requests without an update rectangle retain partial rendering. The existing typography renderer patch remains in place.

The isolated `repaint_probe` executable paints fault pixels into its own presented window using GDI, calls Windows `InvalidateRect`, and compares actual client-area pixels after the normal event loop handles the repaint. No Slint snapshot is taken to validate recovery, since that would redraw the scene independently and could hide the bug. It checks partial, bottom-right, and full invalidation, including boundary pixels to catch fractional-scale rounding fringes.

`artifacts/repaint-before.json` records the unpatched backend failing at 100%, 125%, and 150% scaling. `artifacts/repaint-0.1.1.json` records the corrected backend passing all nine cases across the same three scaling levels. The probe uses an isolated window with no configuration or Discord connection. Original monitor-crossing and snap gestures remain in the manual checklist.

Both workspace crates and the Cargo lockfile now use application version **0.1.1**. The new installer is `release/Presence-Control-0.1.1-windows-x64-setup.exe`. Installer lifecycle verification can use a temporary registration/shortcut identity (`scripts/test-installer.ps1 -Isolated`) while the user's normal application is installed.

Final verification passed: **85 workspace tests**, **nine native repaint cases**, **28 isolated installer lifecycle checks**, and **72 installed native workflow checks**. Clippy with `-D warnings`, formatting, and whitespace checks passed. The installer report is `artifacts/installer-test/c3534d33-4407-4ef2-a37e-e9e89a7d6e7e/installer-report.json`; the installed workflow report is `artifacts/native-smoke/11680500-1abd-46eb-b9eb-cc375a999697/smoke-report.json`. Normal application data remained unchanged during verification.

Both the portable executable and setup expose FileVersion/ProductVersion **0.1.1**. Setup is 6,476,198 bytes; the portable executable is 17,502,720 bytes. All 15 entries in the current `release/SHA256SUMS.txt` were verified, including setup, the patched executable, runtime files, and documentation. The existing installed 0.1.0 copy can be closed and upgraded using the new installer.

The published upstream backend package omits its monorepo Slint test dependency, so its standalone library test harness is unavailable in this workspace. The backport is exercised through the actual backend linked by the application's native pixel probe and smoke run instead.

## Practical limits

The renderer still uses grayscale antialiasing, not WebView2/ClearType RGB subpixel output, so pixel-for-pixel equality with the older application is not established.

Callback smoke checks establish the native host/backend path and persisted behavior. They do not simulate every mouse/keyboard interaction or automatically establish visual fidelity; screenshots require review. Real Discord publishing needs an actual application ID and Discord desktop and has not been verified here. Native import/export dialogs are supplied by the Windows Rust host; their complete manual interaction is outside the callback smoke coverage. Tray behavior is not implemented.
