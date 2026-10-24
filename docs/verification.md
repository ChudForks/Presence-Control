# Verification record

Finalized October 24, 2026, on Windows with Rust 1.98.1 and Slint 1.18.1. The application requires Rust 1.92 or later.

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

## Practical limits

The renderer still uses grayscale antialiasing, not WebView2/ClearType RGB subpixel output, so pixel-for-pixel equality with the older application is not established.

Callback smoke checks establish the native host/backend path and persisted behavior. They do not simulate every mouse/keyboard interaction or automatically establish visual fidelity; screenshots require review. Real Discord publishing needs an actual application ID and Discord desktop and has not been verified here. Native import/export dialogs are supplied by the Windows Rust host; their complete manual interaction is outside the callback smoke coverage. Installer and tray behavior are not implemented.
