# Manual testing checklist

Run `release/presence-control.exe`, or run `cargo run -p presence-desktop` from this repository. The new window is titled **Presence Control — Rust**. The Slint app uses its own `%APPDATA%\dev.presence.control.rust.slint` data directory.

The automated checks already cover 85 Rust tests and 72 native callback/workflow checks. These manual checks cover actual input, Windows dialogs, and real Discord behavior.

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
- Check rounded corners on uploaded images in the editor, live preview, and saved presence cards. The small preview badge should be circular. Check non-square images, changing an image URL, and cards for unselected presences; unsaved artwork should only affect the editor/preview until saved.
- Trigger a status notification such as Stop and leave it open: it should disappear after five seconds. Trigger another notice before that interval ends and check that it gets five seconds of its own; the close button should still dismiss immediately.
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
- Check Applications with no rules and Open applications before scanning: both empty messages should have comfortable padding and stay inside their cards when the window is narrow.
- Create several matching rules and drag their six-dot handles above/below other applications. Check the insertion line, drop outside the list to cancel, and restart to verify the saved order. The first enabled matching rule should win. Focus a handle and check the Up/Down keys, then verify Edit and enable/disable still work.
- Test an optional title filter and process names with/without `.exe`, ignoring case.
- Enable automatic mode and allow up to 15 seconds for checks. Change titles, close applications, and switch matches.
- Go live manually while automatic mode is enabled: manual presence should take priority. Stop disables automatic mode; enable it again to resume matching.

## 6. Time, activity, and archive

- Enable recording during a confirmed automatic presence. It should accumulate application time; a manually started presence should not add recorded application time.
- Stop, disable recording, disconnect Discord, and switch matching applications. Confirm that time pauses or begins a new session appropriately.
- Archive a completed session: cumulative time should remain. Delete a completed session: cumulative time should also remain.
- Archive an application total while recording: its row and cumulative time should remain in Application time, marked In Archive. Finish the recording, then record another session; the same Archive entry should update after each session, while individual archived sessions stay fixed. Repeated Archive actions should not create duplicates.
- Delete an application total after pausing recording: its row should disappear and the saved archive value should remain. A later recording starts a fresh total and leaves that saved value fixed. Delete an archived application total separately: Application time should remain and the archive should stop updating.
- Test event/session/total expansion and archive/delete controls. Deleting the active total and managing an unfinished session should require recording to pause; archiving a live application total is allowed.
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
