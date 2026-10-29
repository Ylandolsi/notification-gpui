//! The notification model and the store the HTTP API and the island share.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Notifications kept in history; older ones fall off.
const HISTORY_CAP: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notification {
    pub id: String,
    pub app: String,
    pub title: String,
    pub body: String,
    pub urgency: Urgency,
    /// How long the toast stays up; `None` means the urgency's default (critical stays until dismissed).
    pub timeout_ms: Option<u64>,
    pub created_ms: u64,
}

impl Notification {
    /// How long the toast shows, or `None` when it stays until dismissed.
    pub fn toast_duration(&self) -> Option<std::time::Duration> {
        let ms = match (self.timeout_ms, self.urgency) {
            (Some(0), _) => return None,
            (Some(ms), _) => ms,
            (None, Urgency::Low) => 3_000,
            (None, Urgency::Normal) => 5_000,
            (None, Urgency::Critical) => return None,
        };
        Some(std::time::Duration::from_millis(ms))
    }
}

/// The body of `POST /v1/notifications`.
#[derive(Debug, Deserialize)]
pub struct NewNotification {
    pub id: Option<String>,
    pub app: Option<String>,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub urgency: Urgency,
    pub timeout_ms: Option<u64>,
}

/// What the server tells the island.
#[derive(Debug, Clone)]
pub enum Cmd {
    /// A new or replaced notification to toast (not sent while Do Not Disturb is on, unless critical).
    Toast(Notification),
    /// History or Do Not Disturb changed; redraw.
    Refresh,
}

#[derive(Default)]
struct Inner {
    /// Newest first.
    history: Vec<Notification>,
    dnd: bool,
    next_id: u64,
}

#[derive(Default)]
pub struct Store(Mutex<Inner>);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Store {
    /// Adds a notification, or replaces the one with the same id in place.
    pub fn post(&self, new: NewNotification) -> Notification {
        let mut inner = self.0.lock().unwrap();
        let id = new.id.unwrap_or_else(|| {
            inner.next_id += 1;
            format!("n{}", inner.next_id)
        });
        let n = Notification {
            id,
            app: new.app.unwrap_or_else(|| "notchd".to_string()),
            title: new.title,
            body: new.body,
            urgency: new.urgency,
            timeout_ms: new.timeout_ms,
            created_ms: now_ms(),
        };
        match inner.history.iter().position(|h| h.id == n.id) {
            Some(i) => inner.history[i] = n.clone(),
            None => {
                inner.history.insert(0, n.clone());
                inner.history.truncate(HISTORY_CAP);
            }
        }
        n
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut inner = self.0.lock().unwrap();
        let before = inner.history.len();
        inner.history.retain(|n| n.id != id);
        inner.history.len() != before
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().history.clear();
    }

    pub fn history(&self) -> Vec<Notification> {
        self.0.lock().unwrap().history.clone()
    }

    pub fn dnd(&self) -> bool {
        self.0.lock().unwrap().dnd
    }

    pub fn set_dnd(&self, on: bool) {
        self.0.lock().unwrap().dnd = on;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(id: Option<&str>, title: &str) -> NewNotification {
        NewNotification {
            id: id.map(str::to_string),
            app: None,
            title: title.into(),
            body: String::new(),
            urgency: Urgency::Normal,
            timeout_ms: None,
        }
    }

    #[test]
    fn same_id_replaces_in_place() {
        let store = Store::default();
        store.post(new(Some("build"), "running"));
        store.post(new(None, "other"));
        store.post(new(Some("build"), "passed"));
        let history = store.history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].title, "passed");
    }

    #[test]
    fn critical_stays_until_dismissed_and_zero_timeout_sticks() {
        let mut n = Store::default().post(new(None, "x"));
        assert!(n.toast_duration().is_some());
        n.urgency = Urgency::Critical;
        assert!(n.toast_duration().is_none());
        n.urgency = Urgency::Normal;
        n.timeout_ms = Some(0);
        assert!(n.toast_duration().is_none());
    }
}
