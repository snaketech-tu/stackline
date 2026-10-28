//! The error-rate alert: counts the errors each opened connection logged in
//! the last few minutes and sends a notification when the count reaches the
//! threshold. It only reads the local log store.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Result;
use serde::Serialize;

use crate::config::Settings;
use crate::store::ErrorRate;

/// Shows a notification on this computer (the desktop app's native ones).
pub trait Notifier: Send + Sync {
    fn notify(&self, title: &str, body: &str) -> Result<()>;
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    pub conn: String,
    pub name: String,
    /// The count is at or above the threshold right now.
    pub active: bool,
    pub count: u64,
    pub window_mins: u32,
    pub threshold: u32,
    /// Unix ms: when the count reached the threshold.
    pub since: Option<i64>,
    /// Unix ms: the last notification for this connection.
    pub notified_at: Option<i64>,
    /// Goes up with every notification, so a browser tab shows each one once.
    pub seq: u64,
    pub top: Option<String>,
    pub top_service: Option<String>,
}

impl Alert {
    pub fn title(&self) -> String {
        format!("Errors on {}", self.name)
    }

    pub fn body(&self) -> String {
        let mut b = format!(
            "{} {} in the last {} minutes (alert at {}).",
            self.count,
            if self.count == 1 { "error" } else { "errors" },
            self.window_mins,
            self.threshold
        );
        if let Some(t) = &self.top {
            b.push_str(&format!("\nMost frequent: {t}"));
            if let Some(s) = &self.top_service {
                b.push_str(&format!(" ({s})"));
            }
        }
        b
    }
}

/// The next state of one connection's alert, and whether to notify.
///
/// A notification goes out when the count reaches the threshold. It goes out
/// again only after the count fell below the threshold and at least one
/// window has passed since the last one, so a flapping rate does not spam.
fn step(prev: Option<&Alert>, next: &mut Alert, now: i64) -> bool {
    let over = next.count >= next.threshold as u64;
    let was = prev.is_some_and(|p| p.active);
    next.notified_at = prev.and_then(|p| p.notified_at);
    next.seq = prev.map_or(0, |p| p.seq);
    next.active = over;
    next.since = if over { prev.filter(|p| p.active).and_then(|p| p.since).or(Some(now)) } else { None };
    let quiet = next.notified_at.is_none_or(|t| now - t >= next.window_mins as i64 * 60_000);
    if over && !was && quiet {
        next.notified_at = Some(now);
        next.seq += 1;
        return true;
    }
    false
}

#[derive(Default)]
pub struct Alerts {
    state: Mutex<HashMap<String, Alert>>,
    notifier: RwLock<Option<Arc<dyn Notifier>>>,
}

impl Alerts {
    pub fn set_notifier(&self, n: Arc<dyn Notifier>) {
        *self.notifier.write().unwrap() = Some(n);
    }

    /// True when notifications are shown natively; otherwise the page shows them.
    pub fn native(&self) -> bool {
        self.notifier.read().unwrap().is_some()
    }

    pub fn list(&self) -> Vec<Alert> {
        let mut v: Vec<Alert> = self.state.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Forgets connections that are no longer open, or everything when alerts are off.
    pub fn retain(&self, open: &[String]) {
        self.state.lock().unwrap().retain(|k, _| open.contains(k));
    }

    /// Records a new count for a connection and notifies when it crosses the threshold.
    pub fn update(&self, conn: &str, name: &str, rate: ErrorRate, settings: &Settings, now: i64) {
        let mut next = Alert {
            conn: conn.to_string(),
            name: name.to_string(),
            active: false,
            count: rate.count,
            window_mins: settings.alert_window_mins,
            threshold: settings.alert_threshold.max(1),
            since: None,
            notified_at: None,
            seq: 0,
            top: rate.top,
            top_service: rate.top_service,
        };
        let fire = {
            let mut map = self.state.lock().unwrap();
            let fire = step(map.get(conn), &mut next, now);
            map.insert(conn.to_string(), next.clone());
            fire
        };
        if fire {
            tracing::info!("{}: {}", next.title(), next.body());
            self.send(&next.title(), &next.body());
        }
    }

    /// Sends a notification natively when that is possible. Returns false when the page has to show it.
    pub fn send(&self, title: &str, body: &str) -> bool {
        let n = self.notifier.read().unwrap().clone();
        match n {
            Some(n) => {
                if let Err(e) = n.notify(title, body) {
                    tracing::warn!("could not show a notification: {e:#}");
                }
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    struct Count(Mutex<Vec<String>>);
    impl Notifier for Count {
        fn notify(&self, title: &str, _: &str) -> Result<()> {
            self.0.lock().unwrap().push(title.to_string());
            Ok(())
        }
    }

    fn rate(count: u64) -> ErrorRate {
        ErrorRate { count, top: Some("Cannot connect to redis".into()), top_service: Some("shop_api".into()) }
    }

    #[test]
    fn notifies_once_per_crossing_and_waits_a_window_before_the_next() {
        let alerts = Alerts::default();
        let sent = Arc::new(Count(Mutex::new(vec![])));
        alerts.set_notifier(sent.clone());
        let s = Settings { alert_enabled: true, alert_window_mins: 5, alert_threshold: 10, ..Default::default() };
        let n = || sent.0.lock().unwrap().len();

        alerts.update("c1", "chronos", rate(3), &s, 0);
        assert_eq!(n(), 0);
        alerts.update("c1", "chronos", rate(12), &s, MIN);
        assert_eq!(n(), 1);
        let a = &alerts.list()[0];
        assert!(a.active);
        assert_eq!((a.since, a.seq), (Some(MIN), 1));
        assert!(a.body().contains("12 errors in the last 5 minutes"), "{}", a.body());

        // Still over: no second notification.
        alerts.update("c1", "chronos", rate(20), &s, 2 * MIN);
        assert_eq!(n(), 1);
        assert_eq!(alerts.list()[0].since, Some(MIN));

        // Dips below and comes back within the window: stays quiet.
        alerts.update("c1", "chronos", rate(4), &s, 3 * MIN);
        assert!(!alerts.list()[0].active);
        alerts.update("c1", "chronos", rate(11), &s, 4 * MIN);
        assert_eq!(n(), 1);

        // Below again, then over after a full window: notifies again.
        alerts.update("c1", "chronos", rate(2), &s, 5 * MIN);
        alerts.update("c1", "chronos", rate(15), &s, 7 * MIN);
        assert_eq!(n(), 2);
        assert_eq!(alerts.list()[0].seq, 2);

        // Each connection has its own alert.
        alerts.update("c2", "homelab", rate(10), &s, 7 * MIN);
        assert_eq!(n(), 3);
        assert_eq!(sent.0.lock().unwrap()[2], "Errors on homelab");
        alerts.retain(&["c2".to_string()]);
        assert_eq!(alerts.list().len(), 1);
    }
}
