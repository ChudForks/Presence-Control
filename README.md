# Presence Control — Rust Complete Rewrite

A native Windows Discord Rich Presence studio built with Rust and Slint. This is a separate Git repository nested in the Presence Control workspace, inherited from **Presence Control Rust Rewrite**. Its application code and backend run in Rust; its native interface is defined in Slint markup and compiled into the executable.

The Slint studio follows the Rust rewrite's black interface, sidebar navigation, saved-presence list, profile editor, Discord-style preview, application rules, activity history, archive, and settings. It does not use a webview, Tauri, React, Node.js, JavaScript, or PowerShell to run the application.

## Install on Windows

Run **`release/Presence-Control-0.1.2-windows-x64-setup.exe`**. Setup supports 64-bit Windows 10 or later, installs for the current Windows account without administrator privileges, and includes the Microsoft C++ runtime. Rust, Node.js, and WebView2 are not required on the destination PC.

The default destination is `%LOCALAPPDATA%\Programs\Presence Control Rust`. Setup creates a **Presence Control (Rust)** Start menu entry and offers an optional desktop shortcut. Close the app and rerun setup to upgrade; the previous installation directory is reused. Remove the app through **Windows Settings → Apps → Installed apps → Presence Control (Rust)** or its `Uninstall.exe`. Upgrades and uninstall preserve profiles, rules, usage history, and archives in the [data directory](#data-and-migration-from-the-rust-rewrite).

This local build is unsigned. The installer and its payload have SHA-256 hashes in `release/SHA256SUMS.txt`.

Silent setup is supported with `/S`. A custom `/D=` directory must be the last argument, with no quotes around its value, including when it contains spaces:

```powershell
Start-Process '.\release\Presence-Control-0.1.2-windows-x64-setup.exe' -ArgumentList '/S /D=C:\Users\you\Apps\Presence Control' -Wait
```

Silent uninstall uses `Uninstall.exe /S`. Setup returns code `2` when it cannot proceed because the application is locked, another setup is running, or Windows is unsupported. It never terminates the running application.

## Build and run

Build on Windows with Rust 1.92 or later using the MSVC toolchain and Microsoft C++ Build Tools/Windows SDK. Cargo downloads the Rust dependencies on the first build. No npm installation or WebView2 runtime is required.

From this directory:

```powershell
cargo run -p presence-desktop
```

Build a release executable:

```powershell
cargo build --release -p presence-desktop
```

The default Cargo output is `target/release/presence-control.exe`. This checkout has a Git-ignored `.cargo/config.toml` that puts build artifacts in the current user's `%LOCALAPPDATA%/PresenceControlRustBuild/target` to conserve space on the project drive. The machine-local Cargo cache setting is not part of the repository and fresh clones use Cargo's normal target directory.

A built portable copy with the latest UI and connection fixes is available in this checkout at **`release/presence-control.exe`**. Close any existing Presence Control window before launching it for normal use.

For portable use on another PC, copy the executable **and all accompanying DLLs** from `release/` together. The installer packages these files automatically.

The build embeds the application icon. Closing the window saves recorded time and disconnects Discord; there is no background tray mode.

The UI uses Slint's native Winit window backend and software renderer. A [pinned renderer patch](vendor/i-slint-renderer-software/LOCAL_CHANGES.md) enables Windows font hinting and display-contrast correction for text throughout the app, including inputs and small helper text. Font weights follow the earlier studio's typography. Application detection uses Windows APIs directly, and Discord transport uses Rust/Tokio named pipes.

Version **0.1.1** also includes a [Windows repaint backport](vendor/i-slint-backend-winit/LOCAL_CHANGES.md). It makes Slint repaint areas invalidated by Windows even when the UI itself has not changed, addressing stale or see-through client areas after window-management transitions. It preserves partial rendering for ordinary UI updates.

## Use

The [manual testing checklist](docs/manual-testing.md) covers real Discord, Windows dialogs, keyboard input, resizing, and persistence. Automated results are recorded in [verification.md](docs/verification.md).

1. Create an application in the [Discord Developer Portal](https://discord.com/developers/applications).
2. Open **Settings** and save the public **Application ID**. A profile can supply an optional override in its **Connection** tab.
3. Edit a presence's display name, activity type, details, state, artwork, and optional buttons, then save it.
4. Open Discord desktop with activity sharing enabled and select **Go live**.

Settings marks unsaved application ID edits. **Go live** also saves a pending global ID when the presence has no override; the Connection tab displays the saved global ID. A presence with its own override keeps using that override independently of pending Settings edits.

**Settings → Appearance** offers **Dark**, **Light**, and **System**. Dark keeps the OLED black default. Light uses bright surfaces and dark text throughout the studio, previews, menus, and dialogs. System follows Windows' application appearance and updates while the app is open. Changes save automatically and preserve unsaved profile and application ID edits. The preference persists across restarts and workspace exports/imports; workspaces created before this setting keep Dark.

Profiles support Playing, Listening to, Watching, and Competing in activity types, elapsed timers, manually configured playback durations, artwork asset keys or HTTPS image URLs, and up to two HTTP/HTTPS buttons. Media playback is not read or controlled.

HTTPS artwork appears in saved presence cards as well as the editor and Discord-style preview. Images use rounded corners; the small overlay badge is circular. Status notifications dismiss automatically after five seconds, and the close button dismisses them immediately. Each new notification starts a fresh timer.

Application rules match exact process names without regard to case, with an optional `.exe` suffix and optional window-title filter. The first enabled matching rule wins. Automatic presence checks run every 15 seconds; manual presence takes priority. Stopping presence also disables automatic mode.

In **App integrations**, drag an application's six-dot handle to change its priority. The highlighted line shows where it will go, and the order saves when you drop it. You can also focus a handle and use the Up/Down arrow keys. The top application has the highest priority.

Templates in display names, details, states, and artwork hover text support `{{app}}`, `{{title}}`, and `{{process}}`. Application names prefer the executable's ProductName version resource, with the executable's filename as a fallback.

Optional application-time recording starts only after a confirmed automatic presence. It pauses on disconnection, manual takeover, disabled recording, or changed application. Activity includes cumulative totals, up to 500 recent sessions, recent events, and a permanent archive. Archiving an application total keeps it in Application time and maintains one corresponding Archive entry, updated after each completed recording. Archiving is available while recording; deleting the active total requires recording to pause. Deleting a total removes its row and leaves its final archive value saved; deleting an archived total stops its archive updates and leaves Application time counting. Archiving or deleting an individual session preserves cumulative time, and archived sessions remain fixed records.

**Application time** counts live, with the current recording's session duration beneath its application name. **Previous sessions** contains completed recordings only: a session appears there with its final duration when recording stops, pauses, switches applications, or disconnects. Checkpoints still preserve recorded time for recovery. Activity rows share fixed, right-aligned time and action columns across totals, events, history, and archives.

Linked application archives persist across restarts and retry failed updates. The total's checkpoint is saved before its archive update; startup repairs an interrupted update from the saved total. Application snapshots saved by older builds remain fixed records. New links never merge or overwrite those older snapshots.

## Data and migration from the Rust rewrite

The Slint app has its own directory:

```text
%APPDATA%\dev.presence.control.rust.slint\
  config.json
  usage.json
  archive.json
```

To use an explicit data directory, set `PRESENCE_CONTROL_DATA_DIR` before starting the app:

```powershell
$env:PRESENCE_CONTROL_DATA_DIR = Join-Path $PWD 'local-data'
cargo run -p presence-desktop
```

To move profiles and rules from **Presence Control Rust Rewrite**, export its workspace in Settings and import that JSON file in the Slint app. Import replaces the Slint configuration and leaves automatic mode off. Export the current Slint workspace first if it contains profiles you want to keep.

Workspace exports contain configuration, not usage history or archives. To migrate history, close both Rust applications, back up the Slint data directory, and copy the corresponding `config.json`, `usage.json`, and `archive.json` together from `%APPDATA%\dev.presence.control.rust\` to the Slint directory. Copying all three preserves rule IDs and their associated totals and sessions. This is replacement, not a history merge. The source application's files are not moved or automatically modified.

Malformed local data is preserved; recovery files receive subsequent writes. On restart, the app resumes the newest valid recovery file while the original remains invalid. A valid original file takes priority. Recent unarchived events are held in memory. The app stores no Discord account token, bot token, or client secret.

## Architecture and verification

```text
crates/presence-core/        Inherited Rust backend, engine, IPC, storage, native detection
crates/presence-desktop/
  ui/                       Compiled Slint interface and components
  src/main.rs               Native event loop and asynchronous backend adapter
  src/editor.rs             Rust drafts, validation, previews, rules, activity projections
  src/native.rs             Data directory, instance guard, Developer Portal action
  src/artwork.rs            Bounded HTTPS image loading and decoding
assets/                     Inherited branding and icon assets
docs/migration.md           Source provenance and behavior inventory
packaging/windows/         NSIS installer, bundled readme, and third-party notice
scripts/build-installer.ps1 Release build, app-local runtime, setup, and checksums
scripts/test-installer.ps1  Isolated Windows install/upgrade/uninstall verification
```

```powershell
cargo test --workspace
cargo test -p i-slint-renderer-software --lib
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p i-slint-renderer-software --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run -p presence-desktop -- --smoke-test
.\scripts\test-repaint.ps1
```

The workspace test suite covers pointer input, drag-and-drop priority and keyboard controls, the native Windows scan, Rust editor transitions, global application ID activation, mock Discord IPC, archive/storage recovery, instance signaling, shutdown failure/retry, rounded artwork and card loading, notification timing, live recording transitions into completed history, and linked application archives across sessions, deletion, restarts, and failed writes. See [verification details](docs/verification.md) for the latest results, verified scope, and remaining checks.

The optional native smoke run opens the actual Slint window and exercises the Rust callback/backend path, then checks persisted data after reopening the backend. It always creates an isolated workspace beneath `artifacts/native-smoke/<run-id>/` and writes a JSON report and screenshots there. Normal application data is not used.

### Rebuild the installer

Install [NSIS 3](https://nsis.sourceforge.io/Download) alongside the Rust/MSVC build prerequisites, then run:

```powershell
.\scripts\build-installer.ps1
.\scripts\test-installer.ps1
```

The build script runs `cargo build --locked --release`, obtains the version and executable path from Cargo, validates x64 binaries, bundles the x64 Microsoft VC runtime from Visual Studio's redistributable directory, and writes setup plus `SHA256SUMS.txt` under `release/`. It discovers NSIS on PATH, in common installation directories, or in an existing Electron builder cache. That cache only supplies the compiler; it adds no Electron dependency to the Rust application. To use explicit tool locations:

```powershell
.\scripts\build-installer.ps1 -NsisCompiler 'C:\Tools\NSIS\makensis.exe' -CrtDirectory 'C:\path\to\x64\Microsoft.VC143.CRT'
```

The lifecycle test requires an account with no existing **Presence Control (Rust)** installation or shortcuts. If the app is already installed, use `.\scripts\test-installer.ps1 -Isolated`: it compiles the same installer source and payload under a temporary registration and shortcut identity, preserving the real installation. Both modes install to an isolated directory with spaces and Unicode, verify registration and shortcuts, test locked-file handling and reinstallation, run the installed application's native smoke checks, and uninstall. They check that normal application data and additional files survive. Reports remain under `artifacts/installer-test/<run-id>/`. Interactive wizard pages and the optional desktop shortcut are included in the [manual checklist](docs/manual-testing.md).

The repaint probe creates its own small native window without reading application data. It damages the actual presented pixels through GDI, asks Windows to invalidate those areas, and verifies recovery with no UI property changes. It checks partial, bottom-right, and full invalidation at 100%, 125%, and 150% Slint scaling. Reports remain under `artifacts/repaint/`. This tests the Windows repaint path rather than Slint's independently rendered snapshots.

`presence-core` is independently usable without the desktop UI:

```powershell
cargo run -p presence-core --example headless -- .\headless-data
```

Windows application detection is platform specific. Real Discord publishing requires Discord desktop and a valid application ID. Automated mock IPC tests do not establish that a particular real Discord account/application has been verified. The project provides a Windows installer and a portable executable; tray mode is not implemented.
