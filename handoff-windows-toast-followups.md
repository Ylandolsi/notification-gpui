# Handoff: Windows toast capture and click-through in notchd

Next session: finish the four open items below. Everything else about this feature is done, committed and pushed to `main`.

## Where things stand

notchd is a Windows notification centre in Rust on gpui (`README.md` covers the product). A daemon shows a dynamic-island pill; notifications come from a localhost HTTP API, a CLI, and now from Windows' own toasts.

Shipped on `main` (read the diffs for detail; do not re-derive them here):

| Commit | What |
|---|---|
| `2685faf` | `src/winnotif.rs` polls `UserNotificationListener` every 500 ms and posts each new toast through `model::publish` as `win-<listener id>` |
| `3664606` | `src/winactivate.rs`: a click on a Windows toast does what the native click does; wired into `src/panel.rs` (toast click and list-row click) |

The user confirmed by hand that clicking a WhatsApp toast in the island opens the exact chat.

## Open items (the task)

1. **Unpackaged apps (Chrome, Edge, most Win32 apps).** `winactivate::packaged_clsid` only handles packaged apps (AUMID `Family!App`, activator CLSID read from `AppxManifest.xml`). For an unpackaged app the activator CLSID is a property on its Start Menu shortcut: `System.AppUserModel.ToastActivatorCLSID`, next to `System.AppUserModel.ID` (the AUMID). Plan: scan the Start Menu `.lnk` files (user and common Programs folders), read both properties through `IShellLink`/`IPropertyStore`, cache AUMID to CLSID, then reuse `call_activator`. Chromium's activator (verified in its source, `chrome/notification_helper/notification_activator.cc`) receives the launch string and forwards it to `chrome.exe` as `--notification-launch-id`. Not tested on this machine. Needs the windows crate features for the property store (`Win32_UI_Shell_PropertiesSystem`, `Win32_System_Com_StructuredStorage`, `Win32_System_Variant`, likely more).
2. **Other toast kinds are untested.** Protocol toasts (`activationType="protocol"`, e.g. Windows Security `windowsdefender://threat`, Snipping Tool `ms-screensketch:edit?...`) go through `open_uri`. Toasts with no `launch` string (PowerShell test toast, Flow Launcher, CodeNotch) fall through to `launch_app`. Neither was seen working. Verify by clicking.
3. **The native popup still shows next to the island.** `RemoveNotification(id)` returns `Ok` and removes the Action Center entry (verified with a throwaway toast), but nobody has checked whether it dismisses an already-visible popup. Test by eye. Candidate design if it works: an opt-in setting, off by default. Other ideas found in research, all weak: Do Not Disturb (Focus Assist re-enables itself per one Q&A thread), a CloudStore quiet-hours key, per-app "show banners" off in Windows Settings (registry value name unverified).

4. **Expand long notifications (requested by the user, not started).** When a notification's text is long it is cut off: in the history list the body line ends in an ellipsis (seen in a screenshot of a Snipping Tool toast, "Enregistrement automatique dans le do..."), and a toast and list row have fixed heights (`LIST_ROW`, `LIST_ROWS` in `src/panel.rs`). Add a way to see the full text, for example expanding a row in the list or growing the toast. Not investigated yet. Questions to settle with the user before building: expand on click or on hover (a click already activates a Windows toast, see `winactivate::activate`, so the two gestures must not clash; an explicit chevron or a different gesture may be needed), whether the island's height animates (it is spring-driven, see `src/spring.rs`), a cap on expanded height with scrolling, and whether it applies to toasts, list rows, or both. Read `text_column` and the `View::Toast` and `View::List` arms in `src/panel.rs` first.

## Verified facts (reuse, do not re-research)

- `UserNotificationListener` works from an unpackaged process on this machine (Windows 11, build 10.0.26300): `GetAccessStatus` returns `Allowed` and `GetNotificationsAsync(Toast)` lists toasts. `NotificationChanged` fails without package identity (`0x80070490`, same in an independent Rust project, foxglove PR #564), hence the polling. `RequestAccessAsync` is never called.
- Notification database: `%LOCALAPPDATA%\Microsoft\Windows\Notifications\wpndatabase.db` (+ `-wal`, `-shm`). `Notification.Id` equals the listener's `UserNotification.Id`. Join `Notification.HandlerId = NotificationHandler.RecordId`; `PrimaryId` is the AUMID; `Payload` is the toast XML as a BLOB whose root `<toast>` carries `launch` and sometimes `activationType`. Read from a copy of all three files.
- Click mechanics that work (packaged app): `CoCreateInstance(ToastActivatorCLSID, CLSCTX_LOCAL_SERVER)` as `INotificationActivationCallback`, then `Activate(aumid, launch, &[])`. `IApplicationActivationManager::ActivateApplication(aumid, launch)` only opens the app, not the chat (WhatsApp is a Windows App SDK app: its manifest declares `desktop:ToastNotificationActivation`, activator server args `----AppNotificationActivated:`).
- WhatsApp: AUMID `5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App`, activator CLSID `bebd6a5e-1b14-4a57-a195-c772d28b6d15`. Its launch string has the shape `action=Click&key=msg...`; contents are private, so do not log them.
- Windows' own popup cannot be blocked from the listener API. There are no action or reply buttons on captured toasts, and the API has no activation call.

## Constraints and preferences from the user

- **No UI Automation** (or other window scraping) for click-through or hiding the popup. The user stopped a probe and said not to follow it. Memory: `feedback-no-ui-automation`.
- **The notification database holds private message text.** Ask before reading it. Earlier the user allowed one WhatsApp row limited to app id, tag, arrival time and the root attributes, with digits masked in output. Keep the same discipline: print only structure, never message text.
- Browser web-push toasts (Facebook): not verified end to end. A DevTools test was started and abandoned; WhatsApp worked, so it was dropped. Chrome and Edge use their own COM activator, which is open item 1.
- The repo owner prefers small, surgical changes and project-native checks (see their global instructions); `cargo test` is the check, plus a hand click for UI behavior since there is no way to click from a script.

## Build and run notes

- Rust host is `x86_64-pc-windows-gnu`. `rusqlite` (feature `bundled`) compiles SQLite from C, and it only built after `C:\msys64\ucrt64\bin` was put first on `PATH` (`mingw64\bin` first made gcc's `cc1` exit silently). Use `export PATH="/c/msys64/ucrt64/bin:$PATH"` before `cargo build`.
- Run the daemon with `target\debug\notchd.exe`; it listens on `127.0.0.1:7878`. CLI: `notchd list`, `notchd notify`, `notchd clear`, `notchd dnd`, `notchd state`.
- A throwaway toast can be fired from PowerShell with `ToastNotificationManager.CreateToastNotifier("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe")`; it appears in notchd as app "Windows PowerShell".
- Pre-existing warning: `Spring::between` is unused (not from this work).

## Research that is settled (do not repeat)

- Notifications Pro (`lwytch/Notifications-Pro`, GPLv3, so reading only, no copying): polls every 2 s plus `NotificationChanged`, suppresses with `RemoveNotification`, falls back to `SetWinEventHook` plus UI Automation. Ruled out by the user.
- Sparse-package identity (self-signed cert, `Add-AppxPackage -ExternalLocation`) would enable `NotificationChanged` but is not needed for polling. Its manifest capability name for the listener was never verified. Skip unless polling proves inadequate.
- `explorer-lite` (claimed sparse package with the listener capability) returned 404; treat as unconfirmed.
- Nothing documented lets one app click another app's delivered toast; the activator-call route above is the working substitute.

## Files

- Design note that started it: `support-windows-notif.md` (its "no buttons" and "can't click" statements are now partly out of date: clicking works; action buttons still do not).
- Project memory (outside the repo, `C:\Users\USER\.claude\projects\c--dev-notchd\memory\`): `project_notchd-windows-toast-capture.md` and `feedback_no-ui-automation.md`.
- Throwaway proof-of-concept scripts lived in the session scratchpad under `toast-spike/` and `tools/` (`toast_activate.ps1`, `wpn_peek.py`, `wpn_launch.py`); they are temporary and superseded by `src/winactivate.rs`, so do not rely on them existing.
