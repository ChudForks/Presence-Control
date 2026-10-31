Presence Control - Native Rust Studio

Run presence-control.exe to create and publish custom Discord Rich Presences.
Discord desktop and a public Discord Application ID are required to publish.
No bot token or client secret is needed.

Setup installs for the current Windows account without administrator privileges.
It creates a Start menu entry named "Presence Control (Rust)". A desktop shortcut
is optional. Run setup again to update the installed application; choose Quit
from the tray menu of any running copy first. No Node.js, Rust, npm, or WebView2 installation is required.
The Microsoft Visual C++ runtime DLLs are included beside the executable.

Saved profiles, rules, activity history, and archives are stored in:
  %APPDATA%\dev.presence.control.rust.slint
An explicit PRESENCE_CONTROL_DATA_DIR environment variable overrides that path.
Upgrading and uninstalling preserve these files.

Use Windows Settings > Apps > Installed apps > Presence Control (Rust) to remove
the application, or run Uninstall.exe in the installation directory.

For unattended installation: setup.exe /S
For a custom directory, put /D= last, without quotes around its value:
  setup.exe /S /D=C:\Users\you\Apps\Presence Control
For unattended removal: Uninstall.exe /S
Exit code 0 means success; code 2 includes a running app or unsupported Windows.

For portable use, keep presence-control.exe and all accompanying DLLs together.

Closing the window keeps Presence Control running in the Windows system tray.
Discord presence, automatic application detection, and time recording continue.
Click the tray icon to reopen, or right-click for Open Presence Control and Quit.
Quit saves recorded time and disconnects Discord before exiting. Unsaved profile
edits prompt for Save, Discard, or Cancel. Launching a second copy also reopens
the existing window.
