//! What a click on a captured Windows toast does: what clicking the native one does. The toast's `launch` string
//! and app id are read from a copy of the notification database (the listener API has neither). A `protocol`
//! toast opens its URI. Otherwise the app's toast activator, a COM class Windows calls on a native click, is
//! called with the launch string, which is how WhatsApp opens the right chat. Failing that, the app is launched.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use windows::Management::Deployment::PackageManager;
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Notifications::INotificationActivationCallback;
use windows::Win32::UI::Shell::{
    AO_NONE, ApplicationActivationManager, IApplicationActivationManager, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{GUID, HSTRING, w};

/// Starts the click on a notification posted by `winnotif` (id `win-<n>`); any other id is ignored.
pub fn activate(notification_id: &str) {
    let Some(id) = notification_id
        .strip_prefix("win-")
        .and_then(|n| n.parse().ok())
    else {
        return;
    };
    std::thread::spawn(move || {
        if let Err(err) = run(id) {
            eprintln!("windows notification click: {err}");
        }
    });
}

struct Toast {
    aumid: String,
    launch: String,
    protocol: bool,
}

fn run(id: u32) -> Result<(), String> {
    let toast = read_toast(id)?;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    if toast.protocol && !toast.launch.is_empty() {
        return open_uri(&toast.launch);
    }
    if let Some(clsid) = activator_clsid(&toast.aumid) {
        match call_activator(&clsid, &toast) {
            Ok(()) => return Ok(()),
            Err(err) => eprintln!("windows notification click: toast activator: {err}"),
        }
    }
    launch_app(&toast)
}

fn open_uri(uri: &str) -> Result<(), String> {
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(uri),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW reports success with a value above 32.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(format!("cannot open {uri}"))
    }
}

fn call_activator(clsid: &GUID, toast: &Toast) -> Result<(), String> {
    unsafe {
        let callback: INotificationActivationCallback =
            CoCreateInstance(clsid, None, CLSCTX_LOCAL_SERVER).map_err(|e| e.message())?;
        callback
            .Activate(
                &HSTRING::from(toast.aumid.as_str()),
                &HSTRING::from(toast.launch.as_str()),
                &[],
            )
            .map_err(|e| e.message())
    }
}

fn launch_app(toast: &Toast) -> Result<(), String> {
    unsafe {
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
                .map_err(|e| e.message())?;
        manager
            .ActivateApplication(
                &HSTRING::from(toast.aumid.as_str()),
                &HSTRING::from(toast.launch.as_str()),
                AO_NONE,
            )
            .map(|_| ())
            .map_err(|e| e.message())
    }
}

/// The toast activator of a packaged app (`<family>!<app>`), from its manifest; cached per app.
fn activator_clsid(aumid: &str) -> Option<GUID> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<GUID>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(found) = cache.lock().unwrap().get(aumid) {
        return *found;
    }
    let found = packaged_clsid(aumid);
    cache.lock().unwrap().insert(aumid.to_string(), found);
    found
}

fn packaged_clsid(aumid: &str) -> Option<GUID> {
    let (family, _) = aumid.split_once('!')?;
    let packages = PackageManager::new()
        .ok()?
        .FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(family))
        .ok()?;
    let root = PathBuf::from(
        packages
            .into_iter()
            .next()?
            .InstalledPath()
            .ok()?
            .to_string(),
    );
    let manifest = std::fs::read_to_string(root.join("AppxManifest.xml")).ok()?;
    GUID::try_from(attr(&manifest, "ToastActivatorCLSID")?.as_str()).ok()
}

fn read_toast(id: u32) -> Result<Toast, String> {
    let source = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("no LOCALAPPDATA")?)
        .join(r"Microsoft\Windows\Notifications");
    // Windows keeps the database open, so the files are read from a copy; the log and shared-memory
    // files carry the newest rows.
    let copy = std::env::temp_dir().join(format!("notchd-wpn-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&copy).map_err(|e| e.to_string())?;
    let result = (|| {
        for (i, name) in ["wpndatabase.db", "wpndatabase.db-wal", "wpndatabase.db-shm"]
            .iter()
            .enumerate()
        {
            if let Err(e) = std::fs::copy(source.join(name), copy.join(name))
                && i == 0
            {
                return Err(e.to_string());
            }
        }
        let db = Connection::open_with_flags(
            copy.join("wpndatabase.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| e.to_string())?;
        db.query_row(
            "SELECT h.PrimaryId, n.Payload FROM Notification n \
             JOIN NotificationHandler h ON n.HandlerId = h.RecordId WHERE n.Id = ?1",
            [id],
            |row| {
                let payload = match row.get_ref(1)? {
                    ValueRef::Blob(b) | ValueRef::Text(b) => {
                        String::from_utf8_lossy(b).into_owned()
                    }
                    _ => String::new(),
                };
                Ok((row.get::<_, String>(0)?, payload))
            },
        )
        .map_err(|e| format!("notification {id} not in the database: {e}"))
    })();
    let _ = std::fs::remove_dir_all(&copy);
    let (aumid, payload) = result?;
    let root = payload
        .find("<toast")
        .and_then(|i| payload[i..].find('>').map(|end| &payload[i..i + end]))
        .unwrap_or("");
    Ok(Toast {
        aumid,
        launch: attr(root, "launch").unwrap_or_default(),
        protocol: attr(root, "activationType").is_some_and(|t| t.eq_ignore_ascii_case("protocol")),
    })
}

/// The value of `name="..."` in a piece of XML, with the entities undone.
fn attr(xml: &str, name: &str) -> Option<String> {
    let start = xml.find(&format!("{name}=\""))? + name.len() + 2;
    let value = &xml[start..start + xml[start..].find('"')?];
    Some(
        value
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attr_undoes_entities() {
        let root = r#"<toast launch="action=Click&amp;key=a%3Ab" activationType="protocol""#;
        assert_eq!(attr(root, "launch").unwrap(), "action=Click&key=a%3Ab");
        assert_eq!(attr(root, "activationType").unwrap(), "protocol");
        assert_eq!(attr(root, "duration"), None);
    }
}
