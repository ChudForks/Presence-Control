# Manual testing checklist

Run `release/presence-control.exe`, or run `cargo run -p presence-desktop` from this repository. The new window is titled **Presence Control — Rust**. The Slint app uses its own `%APPDATA%\dev.presence.control.rust.slint` data directory.

The automated checks already cover 69 Rust tests and 52 native callback/workflow checks. These manual checks cover actual input, Windows dialogs, and real Discord behavior.

## 1. Window and keyboard

- Launch normally: the whole window, title bar, and Go live button should fit above the taskbar.
- Maximize, restore, minimize, and resize. Try approximately 1040×720 and 800×620. Profile cards should scroll horizontally when needed; the form body should scroll while its actions stay visible. On narrower windows the preview moves below the editor.
- Click fields and enter text. Try long text, selection, Ctrl+A, copy/paste, Tab and Shift+Tab.
- Use single clicks to switch sidebar pages, select presences and editor tabs, press buttons, toggle switches, and open/select dropdowns. Switching between controls should take focus and activate the control in the same click. Check Space/Enter and tab arrow keys after clicking too.
- Open activity-type and profile dropdowns. Check mouse selection, arrow keys, and Escape. Check Escape on confirmation dialogs too.

## 2. Profile lifecycle and unsaved edits

- Create a presence, edit it, save, close, and reopen. Saved values should persist.
- Duplicate a presence, including its artwork, timer settings, and two buttons.
- Switch to another profile with unsaved edits. Test **Cancel**, **Discard**, and **Save changes** separately.
- Close with unsaved edits and test the same choices.
- Test the delete confirmation. The last remaining profile should be protected from deletion; deleting another profile should also remove its associated application rules.

## 3. Editor and preview

- Test Playing, Listening to, Watching, and Competing in. Preview text should update while typing.
- Test `{{app}}`, `{{title}}`, and `{{process}}`. Preview uses example metadata until that presence is active with detected metadata.
- Test large/small HTTPS artwork, application asset keys, hover text, and all four fallback themes. Asset keys use theme artwork in the local preview; Discord resolves the actual application asset.
- Add two buttons, edit their labels/URLs, and remove each. A third button should be unavailable; invalid URLs should produce an actionable save error.
- Toggle elapsed time. For Listening/Watching, enable progress and enter a duration such as `3:30`. Invalid durations should be rejected. The app does not read or control a media player.

## 4. Real Discord connection

- Save a public Application ID in Settings, open Discord desktop with activity sharing enabled, and select Go live.
- Verify the actual profile's display text, activity type, artwork, timer/progress, and buttons from another Discord account if possible. Discord may hide your own buttons from you.
- Test a per-profile Application ID override, then clear it to inherit the global ID.
- Enter a global ID in Settings without saving, leave the profile override blank, and select Go live. The ID should be saved and used for Discord; reopening the app should retain it. The Connection tab should show saved values and identify pending Settings edits.
- Enter an invalid global ID draft while a profile has a valid override. Going live with the override should still work, and the unrelated Settings draft should remain unsaved.
- Stop presence: the Discord activity should clear and automatic mode should turn off.
- Close/reopen Discord while live and check the connection/error feedback and ability to go live again.

## 5. Applications and automatic switching

- Scan applications. Confirm process names, titles, and Unicode display correctly; add a detected application to a rule.
- Create two matching rules, change their priority with the arrows, and enable/disable them. The first enabled matching rule should win.
- Test an optional title filter and process names with/without `.exe`, ignoring case.
- Enable automatic mode and allow up to 15 seconds for checks. Change titles, close applications, and switch matches.
- Go live manually while automatic mode is enabled: manual presence should take priority. Stop disables automatic mode; enable it again to resume matching.

## 6. Time, activity, and archive

- Enable recording during a confirmed automatic presence. It should accumulate application time; a manually started presence should not add recorded application time.
- Stop, disable recording, disconnect Discord, and switch matching applications. Confirm that time pauses or begins a new session appropriately.
- Archive a completed session: cumulative time should remain. Delete a completed session: cumulative time should also remain.
- Archive an application total: its saved snapshot should enter the archive and future recording should start a fresh total.
- Test event/session/total expansion and archive/delete controls. Active recording entries should be protected until recording pauses.
- Restart and confirm recorded history and archived entries persist. Recent unarchived event logs are intentionally kept only in memory.

## 7. Native import/export dialogs

- Export a workspace to a JSON file, make a small change, and import the exported file. Review the replacement confirmation/unsaved-edit flow; imported automatic mode should be off.
- Cancel both dialogs and confirm the workspace stays unchanged.
- Try malformed or unrelated JSON. An error should appear and the existing workspace should remain available.
- Configuration exports contain profiles and rules; usage and archive files are separate.

## 8. Exit and second launch

- Launch a second copy using the same data directory. It should bring the existing Slint window forward.
- Close while an automatic presence is recording. Presence should disconnect, recorded time should checkpoint, and the process should exit. There is no tray/background mode.

When reporting an issue, include the section/step, what you expected, what happened, and whether it survives a restart.
