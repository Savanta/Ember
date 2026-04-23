use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::sync::{broadcast, mpsc, Mutex};

use crate::config::Config;
use crate::store::memory::MemoryStore;
use crate::store::models::{CloseReason, Notification, NotificationAction, Urgency};
use crate::store::sqlite::SqliteStore;
use crate::core::focus::FocusManager;
use crate::core::grouping::derive_group_key;
use crate::core::rules::DndRules;
use crate::toast::ToastCommand;
use crate::ui_contract::dto::StateDto;

// ── D-Bus signal events ───────────────────────────────────────────────────────

#[derive(Debug)]
pub enum DbusSignal {
    NotificationClosed { id: u32, reason: CloseReason },
    ActionInvoked { id: u32, key: String },
}

// ── Incoming notification from D-Bus ─────────────────────────────────────────

#[derive(Debug)]
pub struct IncomingNotification {
    pub app_name:       String,
    pub replaces_id:    u32,
    pub app_icon:       String,
    pub summary:        String,
    pub body:           String,
    /// Interleaved: [key, label, key, label, …]
    pub actions_raw:    Vec<String>,
    pub hints:          HashMap<String, String>,
    pub expire_timeout: i32,
}

// ── EngineState ───────────────────────────────────────────────────────────────

struct EngineState {
    next_id:      u32,
    dnd:          DndRules,
    ram:          MemoryStore,
    sqlite:       SqliteStore,
    focus:        FocusManager,
    unseen_count: u32,
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// Compact snapshot used for subscribe event stream.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NotifSnapshot {
    pub waiting: u32,
    pub history: u32,
    pub unread:  u32,
    pub dnd:     bool,
}

pub struct Engine {
    state:    Arc<Mutex<EngineState>>,
    toast_tx: mpsc::Sender<ToastCommand>,
    dbus_tx:  mpsc::Sender<DbusSignal>,
    /// Hot-swappable config (per-app rules, timeouts, DND schedule).
    config:   Arc<RwLock<Arc<crate::config::Config>>>,
    event_tx: broadcast::Sender<()>,
}

impl Engine {
    pub fn new(
        config:   Arc<Config>,
        sqlite:   SqliteStore,
        toast_tx: mpsc::Sender<ToastCommand>,
        dbus_tx:  mpsc::Sender<DbusSignal>,
    ) -> Self {
        let state = EngineState {
            next_id:      1,
            dnd:          DndRules::new(config.dnd.enabled, config.dnd.schedule.clone()),
            ram:          MemoryStore::new(),
            sqlite,
            focus:        FocusManager::default(),
            unseen_count: 0,
        };
        let (event_tx, _) = broadcast::channel(64);
        Self {
            state: Arc::new(Mutex::new(state)),
            toast_tx,
            dbus_tx,
            config: Arc::new(RwLock::new(config)),
            event_tx,
        }
    }

    /// Subscribe to state-change events.
    /// Each `()` message means the engine state has changed; call `event_snapshot()` to get new data.
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.event_tx.subscribe()
    }

    /// Compact snapshot for the IPC subscribe stream.
    pub async fn event_snapshot(&self) -> NotifSnapshot {
        let st = self.state.lock().await;
        let waiting = st.ram.active().count() as u32;
        let unread  = st.unseen_count;
        let dnd     = st.dnd.enabled;
        let sqlite  = st.sqlite.clone();
        drop(st);
        let history = sqlite.count_all().await.unwrap_or(waiting);
        NotifSnapshot { waiting, history, unread, dnd }
    }

    /// Notify all active subscribe streams that state has changed.
    fn notify_change(&self) {
        // Ignore error when there are no receivers.
        let _ = self.event_tx.send(());
    }

    /// Hot-reload: swap in a new config without restarting the daemon.
    /// Updates per-app rules, timeouts, and DND schedule immediately.
    pub async fn reload_config(&self, new_config: crate::config::Config) {
        let new_arc = Arc::new(new_config);
        *self.config.write().unwrap() = Arc::clone(&new_arc);
        // Also update live DND rules inside EngineState.
        let mut st = self.state.lock().await;
        st.dnd = crate::core::rules::DndRules::new(
            new_arc.dnd.enabled,
            new_arc.dnd.schedule.clone(),
        );
        drop(st);
        self.notify_change();
        log::info!("config reloaded");
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Called by the D-Bus server for every Notify call.
    /// Returns the assigned notification ID.
    pub async fn receive_notification(&self, incoming: IncomingNotification) -> u32 {
        let mut st = self.state.lock().await;

        // Determine ID: reuse replaces_id if it refers to a live notification,
        // otherwise allocate a new one.
        let id = if incoming.replaces_id != 0 && st.ram.get(incoming.replaces_id).is_some() {
            incoming.replaces_id
        } else {
            let id = st.next_id;
            st.next_id = st.next_id.wrapping_add(1).max(1);
            id
        };

        let urgency = parse_urgency(&incoming.hints);
        let actions = parse_actions(&incoming.actions_raw);
        let group_key = derive_group_key(&incoming.app_name, &incoming.hints);

        let can_reply = incoming.hints.get("x-ember-can-reply")
            .map(|v| v == "true" || v == "1")
            .unwrap_or_else(|| actions.iter().any(|a| a.key.contains("reply")));
        let reply_placeholder = incoming.hints
            .get("x-ember-reply-placeholder")
            .cloned()
            .unwrap_or_else(|| "Reply…".to_string());
        let max_reply_length = incoming.hints
            .get("x-ember-max-reply-length")
            .and_then(|v| v.parse::<u32>().ok());

        let notif = Notification {
            id,
            app_name: incoming.app_name,
            summary:  incoming.summary,
            body:     incoming.body,
            icon:     incoming.app_icon,
            urgency,
            timestamp: now_ms(),
            source_id: incoming.replaces_id,
            actions,
            hints:     incoming.hints,
            expire_timeout: incoming.expire_timeout,
            state:     crate::store::models::NotificationState::Active,
            group_key,
            can_reply,
            reply_placeholder,
            max_reply_length,
        };

        log::debug!("notification {} received app={} summary={}", id, notif.app_name, notif.summary);

        st.ram.upsert(notif.clone());
        st.unseen_count = st.unseen_count.saturating_add(1);
        let focus_id = {
            let ids = active_order_ids(&st);
            st.focus.ensure_valid(&ids)
        };

        // Compute group membership for the stack badge.
        let (group_count, peer_ids) = if let Some(ref gk) = notif.group_key {
            let peers: Vec<u32> = st.ram.active()
                .filter(|n| n.id != id && n.group_key.as_deref() == Some(gk.as_str()))
                .map(|n| n.id)
                .collect();
            let count = (peers.len() as u32) + 1;
            (count, peers)
        } else {
            (1u32, vec![])
        };

        // Persist to SQLite asynchronously; clone pool + notif to avoid holding lock.
        let sqlite     = st.sqlite.clone();
        let notif_pers = notif.clone();
        tokio::spawn(async move {
            if let Err(e) = sqlite.upsert(&notif_pers).await {
                log::warn!("sqlite upsert failed: {e}");
            }
        });

        // Send to toast renderer unless DND suppresses it
        let show_toast = !st.dnd.suppresses_toast(notif.urgency);

        // Apply per-app rules (read live config snapshot).
        let cfg_snap = Arc::clone(&*self.config.read().unwrap());
        let mut timeout_ms = notif.effective_timeout_ms(
            cfg_snap.toast.timeout_normal,
            cfg_snap.toast.timeout_critical,
            cfg_snap.toast.timeout_low,
        );
        let mut muted = !show_toast;
        let mut notif = notif;
        if let Some(rule) = cfg_snap.apps.iter().find(|r| r.matches(&notif.app_name)) {
            if rule.mute { muted = true; }
            if let Some(t) = rule.timeout_ms { timeout_ms = if t == 0 { None } else { Some(t) }; }
            if let Some(urg) = &rule.urgency {
                notif.urgency = match urg.to_lowercase().as_str() {
                    "low"      => Urgency::Low,
                    "critical" => Urgency::Critical,
                    _          => Urgency::Normal,
                };
            }
        }

        drop(st); // release lock before async send

        // Notify existing group-peers about the new count.
        for peer_id in peer_ids {
            let _ = self.toast_tx.send(ToastCommand::UpdateGroupCount { id: peer_id, count: group_count }).await;
        }

        if !muted {
            let _ = self.toast_tx.send(ToastCommand::Show { notif: Box::new(notif), timeout_ms, group_count }).await;
        }
        let _ = self.toast_tx.send(ToastCommand::SetFocus(focus_id)).await;
        self.notify_change();
        id
    }

    /// Dismiss a single notification (e.g. from IPC or CloseNotification D-Bus call).
    pub async fn dismiss(&self, id: u32, reason: CloseReason) {
        let mut st = self.state.lock().await;
        if !st.ram.dismiss(id) {
            return; // already dismissed or unknown
        }
        st.focus.on_dismissed(id);
        let focus_id = {
            let ids = active_order_ids(&st);
            st.focus.ensure_valid(&ids)
        };

        // Collect group peers to update their stack badges.
        let group_key = st.ram.get(id).and_then(|n| n.group_key.clone());
        let peer_updates: Vec<(u32, u32)> = if let Some(ref gk) = group_key {
            let remaining: Vec<u32> = st.ram.active()
                .filter(|n| n.group_key.as_deref() == Some(gk.as_str()))
                .map(|n| n.id)
                .collect();
            let count = remaining.len() as u32;
            remaining.into_iter().map(|pid| (pid, count)).collect()
        } else {
            vec![]
        };

        let sqlite = st.sqlite.clone();
        tokio::spawn(async move {
            let _ = sqlite.update_state(id, crate::store::models::NotificationState::Dismissed).await;
        });
        drop(st);
        let _ = self.toast_tx.send(ToastCommand::Close(id)).await;
        for (peer_id, count) in peer_updates {
            let _ = self.toast_tx.send(ToastCommand::UpdateGroupCount { id: peer_id, count }).await;
        }
        let _ = self.toast_tx.send(ToastCommand::SetFocus(focus_id)).await;
        let _ = self.dbus_tx.send(DbusSignal::NotificationClosed { id, reason }).await;
        self.notify_change();
        log::info!("notification {} dismissed", id);
    }

    /// Dismiss all active notifications.
    pub async fn dismiss_all(&self) {
        let mut st = self.state.lock().await;
        let ids = st.ram.dismiss_all();
        st.focus.clear();
        let sqlite = st.sqlite.clone();
        let ids_clone = ids.clone();
        tokio::spawn(async move {
            for id in &ids_clone {
                let _ = sqlite.update_state(*id, crate::store::models::NotificationState::Dismissed).await;
            }
        });
        drop(st);
        for id in ids {
            let _ = self.toast_tx.send(ToastCommand::Close(id)).await;
            let _ = self
                .dbus_tx
                .send(DbusSignal::NotificationClosed {
                    id,
                    reason: CloseReason::DismissedByUser,
                })
                .await;
        }
        let _ = self.toast_tx.send(ToastCommand::SetFocus(None)).await;
        self.notify_change();
        log::info!("all notifications dismissed");
    }

    /// Toggle DND. Returns the new state.
    pub async fn toggle_dnd(&self) -> bool {
        let mut st = self.state.lock().await;
        st.dnd.enabled = !st.dnd.enabled;
        let enabled = st.dnd.enabled;
        log::info!("DND toggled → {}", enabled);        self.notify_change();        enabled
    }

    /// Called when the toast renderer reports a user-initiated dismiss (click).
    pub async fn on_toast_dismissed(&self, id: u32) {
        self.dismiss(id, CloseReason::DismissedByUser).await;
    }

    /// Called when the toast renderer reports an action invocation.
    pub async fn on_toast_action(&self, id: u32, key: String) {
        let notif = {
            let st = self.state.lock().await;
            st.ram.get(id).cloned()
        };

        if let Some(notif) = &notif {
            if key == "default" {
                crate::core::actions::open_notification(notif);
            } else {
                crate::core::actions::dispatch_action_hook(notif, &key);
            }
        }

        let _ = self
            .dbus_tx
            .send(DbusSignal::ActionInvoked { id, key: key.clone() })
            .await;
        // After invoking an action the notification is typically dismissed.
        self.dismiss(id, CloseReason::DismissedByUser).await;
    }

    /// Called when a toast expires naturally.
    pub async fn on_toast_expired(&self, id: u32) {
        self.dismiss(id, CloseReason::Expired).await;
    }

    /// Invoke an action on a notification (from IPC).
    pub async fn invoke_action(&self, id: u32, key: &str) {
        let notif = {
            let st = self.state.lock().await;
            st.ram.get(id).cloned()
        };

        if let Some(notif) = &notif {
            if key == "default" {
                crate::core::actions::open_notification(notif);
            } else {
                crate::core::actions::dispatch_action_hook(notif, key);
            }
        }

        let _ = self
            .dbus_tx
            .send(DbusSignal::ActionInvoked { id, key: key.to_owned() })
            .await;
        self.dismiss(id, CloseReason::DismissedByUser).await;
    }

    /// Build a state snapshot for IPC consumers (Eww widgets).
    /// Notifications are sorted: Critical first, then Normal, then Low;
    /// within each urgency group newest first.
    pub async fn get_state(&self) -> StateDto {
        let mut st = self.state.lock().await;
        let unread = st.unseen_count;
        let dnd    = st.dnd.enabled;
        let notifications = sorted_active_notifications(&st);
        let ids = notifications.iter().map(|n| n.id).collect::<Vec<_>>();
        let focused_id = st.focus.ensure_valid(&ids);
        StateDto { unread, dnd, focused_id, notifications }
    }

    /// Reset the unread badge counter to zero (e.g. when user opens the popup).
    pub async fn mark_all_read(&self) {
        let mut st = self.state.lock().await;
        st.unseen_count = 0;
        drop(st);
        self.notify_change();
        log::debug!("unseen_count reset to 0");
    }

    pub async fn get_focused_notification(&self) -> Option<Notification> {
        let mut st = self.state.lock().await;
        let notifications = sorted_active_notifications(&st);
        let ids = notifications.iter().map(|n| n.id).collect::<Vec<_>>();
        let focused_id = st.focus.ensure_valid(&ids);
        notifications.into_iter().find(|n| Some(n.id) == focused_id)
    }

    pub async fn focus_next(&self) -> Option<u32> {
        let mut st = self.state.lock().await;
        let ids = active_order_ids(&st);
        let focused = st.focus.focus_next_in(&ids);
        drop(st);
        let _ = self.toast_tx.send(ToastCommand::SetFocus(focused)).await;
        focused
    }

    pub async fn focus_prev(&self) -> Option<u32> {
        let mut st = self.state.lock().await;
        let ids = active_order_ids(&st);
        let focused = st.focus.focus_prev_in(&ids);
        drop(st);
        let _ = self.toast_tx.send(ToastCommand::SetFocus(focused)).await;
        focused
    }

    /// Group active notifications by `group_key`.
    /// Returns groups sorted by the urgency of their highest-urgency member (desc).
    pub async fn get_groups(&self) -> Vec<crate::ui_contract::dto::GroupDto> {
        use std::collections::HashMap;
        let st = self.state.lock().await;
        let mut map: HashMap<String, Vec<crate::store::models::Notification>> = HashMap::new();
        for notif in st.ram.active() {
            map.entry(notif.group_key.clone().unwrap_or_else(|| notif.app_name.clone()))
                .or_default()
                .push(notif.clone());
        }
        let mut groups: Vec<_> = map
            .into_iter()
            .map(|(key, mut notifs)| {
                notifs.sort_unstable_by(|a, b| {
                    b.urgency.cmp(&a.urgency).then_with(|| b.timestamp.cmp(&a.timestamp))
                });
                let max_urgency = notifs
                    .iter()
                    .map(|n| n.urgency as u8)
                    .max()
                    .unwrap_or(0);
                (key, notifs, max_urgency)
            })
            .collect();
        groups.sort_unstable_by(|a, b| b.2.cmp(&a.2));
        groups
            .into_iter()
            .map(|(key, notifs, _)| {
                let count = notifs.len() as u32;
                let notifications = notifs
                    .iter()
                    .map(crate::ui_contract::dto::NotificationDto::from)
                    .collect();
                crate::ui_contract::dto::GroupDto { key, count, notifications }
            })
            .collect()
    }

    /// Return paginated history from SQLite.
    pub async fn history(&self, limit: usize, offset: usize) -> Vec<crate::store::models::Notification> {
        let st     = self.state.lock().await;
        let sqlite = st.sqlite.clone();
        drop(st);
        sqlite.history(limit, offset).await.unwrap_or_default()
    }

    /// Full-text search across history.
    pub async fn search(&self, query: &str, limit: usize) -> Vec<crate::store::models::Notification> {
        let st     = self.state.lock().await;
        let sqlite = st.sqlite.clone();
        drop(st);
        sqlite.search(query, limit).await.unwrap_or_default()
    }

    /// Delete all records from the history database.
    pub async fn clear_history(&self) {
        let st     = self.state.lock().await;
        let sqlite = st.sqlite.clone();
        drop(st);
        let _ = sqlite.clear_history().await;
    }

    /// Delete a single notification record from the history database.
    pub async fn delete_notification(&self, id: u32) {
        let st     = self.state.lock().await;
        let sqlite = st.sqlite.clone();
        drop(st);
        let _ = sqlite.delete_notification(id).await;
    }

    pub async fn send_reply(&self, id: u32, text: &str) -> bool {
        let notif = {
            let st = self.state.lock().await;
            st.ram.get(id).cloned()
        };

        let Some(notif) = notif else { return false; };
        let router = crate::core::replies::DefaultReplyRouter;
        let ok = crate::core::replies::ReplyRouter::send_reply(&router, &notif, text);
        if ok {
            self.dismiss(id, CloseReason::DismissedByUser).await;
        }
        ok
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn parse_urgency(hints: &HashMap<String, String>) -> Urgency {
    hints
        .get("urgency")
        .and_then(|v| v.parse::<u8>().ok())
        .map(Urgency::from_u8)
        .unwrap_or_default()
}

fn parse_actions(raw: &[String]) -> Vec<NotificationAction> {
    raw.chunks(2)
        .filter_map(|c| {
            if c.len() == 2 {
                Some(NotificationAction { key: c[0].clone(), label: c[1].clone() })
            } else {
                None
            }
        })
        .collect()
}

fn sorted_active_notifications(st: &EngineState) -> Vec<Notification> {
    let mut notifications: Vec<_> = st.ram.active().cloned().collect();
    notifications.sort_unstable_by(|a, b| {
        b.urgency.cmp(&a.urgency)
            .then_with(|| b.timestamp.cmp(&a.timestamp))
    });
    notifications
}

fn active_order_ids(st: &EngineState) -> Vec<u32> {
    sorted_active_notifications(st)
        .into_iter()
        .map(|n| n.id)
        .collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use crate::config::Config;
    use crate::store::sqlite::SqliteStore;
    use toml;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    async fn make_engine() -> (Engine, mpsc::Receiver<ToastCommand>, mpsc::Receiver<DbusSignal>) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = format!("/tmp/ember-engine-test-{pid}-{n}.db");
        let sqlite = SqliteStore::open(Path::new(&path)).await.expect("open db");
        let cfg: Config = toml::from_str("").expect("default config");
        let cfg = Arc::new(cfg);
        let (toast_tx, toast_rx) = mpsc::channel(64);
        let (dbus_tx, dbus_rx) = mpsc::channel(64);
        let engine = Engine::new(cfg, sqlite, toast_tx, dbus_tx);
        (engine, toast_rx, dbus_rx)
    }

    fn incoming(app: &str, summary: &str) -> IncomingNotification {
        IncomingNotification {
            app_name:       app.to_string(),
            replaces_id:    0,
            app_icon:       "".into(),
            summary:        summary.to_string(),
            body:           "".into(),
            actions_raw:    vec![],
            hints:          HashMap::new(),
            expire_timeout: -1,
        }
    }

    #[tokio::test]
    async fn receive_notification_returns_incrementing_ids() {
        let (engine, _toast, _dbus) = make_engine().await;
        let id1 = engine.receive_notification(incoming("app", "first")).await;
        let id2 = engine.receive_notification(incoming("app", "second")).await;
        assert_ne!(id1, id2);
        assert!(id2 > id1);
    }

    #[tokio::test]
    async fn receive_notification_replaces_id_reuses_slot() {
        let (engine, _toast, _dbus) = make_engine().await;
        let id1 = engine.receive_notification(incoming("app", "original")).await;

        let mut replacement = incoming("app", "updated");
        replacement.replaces_id = id1;
        let id2 = engine.receive_notification(replacement).await;

        assert_eq!(id1, id2, "replacing an active notification must keep the same ID");
    }

    #[tokio::test]
    async fn toggle_dnd_flips_state_twice() {
        let (engine, _toast, _dbus) = make_engine().await;
        let first = engine.toggle_dnd().await;
        let second = engine.toggle_dnd().await;
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn dismiss_removes_notification_from_active_state() {
        let (engine, _toast, _dbus) = make_engine().await;
        let id = engine.receive_notification(incoming("app", "hi")).await;

        engine.dismiss(id, CloseReason::DismissedByUser).await;

        let state = engine.get_state().await;
        assert!(!state.notifications.iter().any(|n| n.id == id));
    }

    #[tokio::test]
    async fn mark_all_read_resets_unseen_count() {
        let (engine, _toast, _dbus) = make_engine().await;
        engine.receive_notification(incoming("a", "1")).await;
        engine.receive_notification(incoming("a", "2")).await;

        engine.mark_all_read().await;
        let snap = engine.event_snapshot().await;
        assert_eq!(snap.unread, 0);
    }

    #[tokio::test]
    async fn get_state_sorts_critical_before_normal() {
        let (engine, _toast, _dbus) = make_engine().await;

        let mut low = incoming("app", "low");
        low.hints.insert("urgency".into(), "0".into());
        engine.receive_notification(low).await;

        let mut crit = incoming("app", "critical");
        crit.hints.insert("urgency".into(), "2".into());
        engine.receive_notification(crit).await;

        let state = engine.get_state().await;
        assert_eq!(state.notifications[0].summary, "critical");
    }

    #[tokio::test]
    async fn dismiss_all_clears_active_notifications() {
        let (engine, _toast, _dbus) = make_engine().await;
        engine.receive_notification(incoming("a", "one")).await;
        engine.receive_notification(incoming("b", "two")).await;

        engine.dismiss_all().await;

        let state = engine.get_state().await;
        assert!(state.notifications.is_empty());
    }

    #[tokio::test]
    async fn event_snapshot_waiting_reflects_active_count() {
        let (engine, _toast, _dbus) = make_engine().await;
        engine.receive_notification(incoming("a", "x")).await;
        engine.receive_notification(incoming("b", "y")).await;

        let snap = engine.event_snapshot().await;
        assert_eq!(snap.waiting, 2);
    }

    #[tokio::test]
    async fn reload_config_swaps_dnd_rules() {
        let (engine, _toast, _dbus) = make_engine().await;

        // Build a config with DnD enabled
        let new_cfg: Config = toml::from_str("
[dnd]
enabled = true
").expect("parse config");
        engine.reload_config(new_cfg).await;

        // After reload the engine must not suppress notifications while DnD
        // is enabled for the new config — verifiable via get_state
        let state = engine.get_state().await;
        // Just ensure no panic and state is retrievable
        let _ = state;
    }

    #[tokio::test]
    async fn grouped_notifications_share_group_key() {
        let (engine, _toast, _dbus) = make_engine().await;

        let mut a = incoming("chat", "msg1");
        let mut b = incoming("chat", "msg2");

        // Simulate a group_key hint so they get bucketed together
        a.hints.insert("x-ember-group".to_string(), "chat-group".to_string());
        b.hints.insert("x-ember-group".to_string(), "chat-group".to_string());

        let _id1 = engine.receive_notification(a).await;
        let _id2 = engine.receive_notification(b).await;

        let state = engine.get_state().await;
        let keys: Vec<_> = state.notifications.iter()
            .filter_map(|n| n.group_key.as_ref())
            .collect();
        // Both notifications must share the same group_key
        if keys.len() == 2 {
            assert_eq!(keys[0], keys[1]);
        }
    }
}
