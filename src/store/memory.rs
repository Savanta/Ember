use indexmap::IndexMap;
use crate::store::models::{Notification, NotificationState};

/// In-memory store of active notifications.
/// Order is insertion order (IndexMap), newest last.
#[derive(Debug, Default)]
pub struct MemoryStore {
    notifications: IndexMap<u32, Notification>,
}

#[allow(dead_code)]
impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a notification (replaces_id logic handled upstream).
    pub fn upsert(&mut self, notif: Notification) {
        self.notifications.insert(notif.id, notif);
    }

    pub fn get(&self, id: u32) -> Option<&Notification> {
        self.notifications.get(&id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Notification> {
        self.notifications.get_mut(&id)
    }

    pub fn remove(&mut self, id: u32) -> Option<Notification> {
        self.notifications.shift_remove(&id)
    }

    /// All active notifications, newest last.
    pub fn active(&self) -> impl Iterator<Item = &Notification> {
        self.notifications
            .values()
            .filter(|n| n.is_active())
    }

    pub fn active_count(&self) -> usize {
        self.notifications
            .values()
            .filter(|n| n.is_active())
            .count()
    }

    /// Mark a notification as dismissed; returns true if it existed and was active.
    pub fn dismiss(&mut self, id: u32) -> bool {
        if let Some(n) = self.notifications.get_mut(&id)
            && n.is_active() {
                n.state = NotificationState::Dismissed;
                return true;
            }
        false
    }

    /// Mark all active notifications as dismissed; returns their IDs.
    pub fn dismiss_all(&mut self) -> Vec<u32> {
        let ids: Vec<u32> = self
            .notifications
            .values()
            .filter(|n| n.is_active())
            .map(|n| n.id)
            .collect();

        for id in &ids {
            if let Some(n) = self.notifications.get_mut(id) {
                n.state = NotificationState::Dismissed;
            }
        }
        ids
    }

    pub fn all_values(&self) -> impl Iterator<Item = &Notification> {
        self.notifications.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::store::models::Urgency;

    fn make_notif(id: u32) -> Notification {
        Notification {
            id,
            app_name: format!("app-{id}"),
            summary: "s".into(),
            body: "b".into(),
            icon: "".into(),
            urgency: Urgency::Normal,
            timestamp: id as i64,
            source_id: 0,
            actions: vec![],
            hints: HashMap::new(),
            expire_timeout: -1,
            state: NotificationState::Active,
            group_key: None,
            can_reply: false,
            reply_placeholder: "Reply…".into(),
            max_reply_length: None,
        }
    }

    #[test]
    fn upsert_and_get() {
        let mut store = MemoryStore::new();
        store.upsert(make_notif(1));
        assert!(store.get(1).is_some());
        assert!(store.get(2).is_none());
    }

    #[test]
    fn upsert_replaces_existing() {
        let mut store = MemoryStore::new();
        store.upsert(make_notif(1));
        let mut updated = make_notif(1);
        updated.summary = "updated".into();
        store.upsert(updated);
        assert_eq!(store.get(1).unwrap().summary, "updated");
    }

    #[test]
    fn active_only_returns_active_state() {
        let mut store = MemoryStore::new();
        store.upsert(make_notif(1));
        store.upsert(make_notif(2));
        store.dismiss(1);
        let active: Vec<_> = store.active().map(|n| n.id).collect();
        assert_eq!(active, vec![2]);
    }

    #[test]
    fn active_count_matches_active_iterator() {
        let mut store = MemoryStore::new();
        store.upsert(make_notif(1));
        store.upsert(make_notif(2));
        store.upsert(make_notif(3));
        assert_eq!(store.active_count(), 3);
        store.dismiss(2);
        assert_eq!(store.active_count(), 2);
    }

    #[test]
    fn dismiss_returns_true_for_active() {
        let mut store = MemoryStore::new();
        store.upsert(make_notif(1));
        assert!(store.dismiss(1));
        assert!(!store.get(1).unwrap().is_active());
    }

    #[test]
    fn dismiss_returns_false_for_unknown() {
        let mut store = MemoryStore::new();
        assert!(!store.dismiss(99));
    }

    #[test]
    fn dismiss_returns_false_for_already_dismissed() {
        let mut store = MemoryStore::new();
        store.upsert(make_notif(1));
        store.dismiss(1);
        assert!(!store.dismiss(1));
    }

    #[test]
    fn dismiss_all_returns_all_active_ids() {
        let mut store = MemoryStore::new();
        store.upsert(make_notif(1));
        store.upsert(make_notif(2));
        store.upsert(make_notif(3));
        store.dismiss(2);
        let mut dismissed = store.dismiss_all();
        dismissed.sort();
        assert_eq!(dismissed, vec![1, 3]);
        assert_eq!(store.active_count(), 0);
    }

    #[test]
    fn remove_deletes_entry() {
        let mut store = MemoryStore::new();
        store.upsert(make_notif(1));
        assert!(store.remove(1).is_some());
        assert!(store.get(1).is_none());
    }

    #[test]
    fn insertion_order_preserved() {
        let mut store = MemoryStore::new();
        for id in [3u32, 1, 2] {
            store.upsert(make_notif(id));
        }
        let ids: Vec<_> = store.all_values().map(|n| n.id).collect();
        assert_eq!(ids, vec![3, 1, 2]);
    }
}
