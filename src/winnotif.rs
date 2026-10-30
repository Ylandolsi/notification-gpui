//! Windows' own toasts (Chrome, Slack, Outlook, ...) shown in the island. The listener's access is read, never
//! requested, and its arrival event needs package identity, so the notification centre is polled instead; this
//! works from an unpackaged process. Windows still shows its own popup next to the island.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};

use crate::model::{Cmd, NewNotification, Store, Urgency, publish};

const POLL: Duration = Duration::from_millis(500);

/// Starts the poller; it ends at once, with a message, when Windows does not give the app access.
pub fn spawn(store: Arc<Store>, tx: flume::Sender<Cmd>) {
    std::thread::spawn(move || {
        if let Err(err) = run(&store, &tx) {
            eprintln!("windows notifications unavailable: {err}");
        }
    });
}

fn run(store: &Store, tx: &flume::Sender<Cmd>) -> windows::core::Result<()> {
    let listener = UserNotificationListener::Current()?;
    if listener.GetAccessStatus()? != UserNotificationListenerAccessStatus::Allowed {
        eprintln!(
            "windows notifications: access not allowed; enable it under Settings > Privacy & security > Notifications"
        );
        return Ok(());
    }
    // What is already in the notification centre is history, not news.
    let mut seen: HashSet<u32> = toasts(&listener)?.iter().filter_map(|n| n.Id().ok()).collect();
    loop {
        std::thread::sleep(POLL);
        // A failed read (the service restarting, say) is retried on the next tick.
        let Ok(all) = toasts(&listener) else { continue };
        let mut current = HashSet::new();
        for n in all {
            let Ok(id) = n.Id() else { continue };
            current.insert(id);
            if seen.insert(id) {
                publish(store, tx, to_new(id, &n));
            }
        }
        // Dismissed toasts drop out, so an id Windows reuses later counts as new again.
        seen = current;
    }
}

fn toasts(listener: &UserNotificationListener) -> windows::core::Result<Vec<UserNotification>> {
    let all = listener.GetNotificationsAsync(NotificationKinds::Toast)?.get()?;
    Ok((0..all.Size()?).filter_map(|i| all.GetAt(i).ok()).collect())
}

fn to_new(id: u32, n: &UserNotification) -> NewNotification {
    let app = n
        .AppInfo()
        .and_then(|a| a.DisplayInfo())
        .and_then(|d| d.DisplayName())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let mut lines = texts(n).unwrap_or_default().into_iter();
    NewNotification {
        id: Some(format!("win-{id}")),
        app: Some(app).filter(|a| !a.is_empty()),
        title: lines.next().unwrap_or_default(),
        body: lines.collect::<Vec<_>>().join("\n"),
        urgency: Urgency::Normal,
        timeout_ms: None,
    }
}

fn texts(n: &UserNotification) -> windows::core::Result<Vec<String>> {
    let binding = n
        .Notification()?
        .Visual()?
        .GetBinding(&KnownNotificationBindings::ToastGeneric()?)?;
    let elements = binding.GetTextElements()?;
    (0..elements.Size()?)
        .map(|i| Ok(elements.GetAt(i)?.Text()?.to_string()))
        .collect()
}
