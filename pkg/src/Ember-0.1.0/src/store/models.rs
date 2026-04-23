use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Urgency ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
#[derive(Default)]
pub enum Urgency {
    Low      = 0,
    #[default]
    Normal   = 1,
    Critical = 2,
}

impl Urgency {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Low,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }
}


// ── NotificationState ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum NotificationState {
    Active    = 0,
    Dismissed = 1,
    Expired   = 2,
}

impl NotificationState {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Dismissed,
            2 => Self::Expired,
            _ => Self::Active,
        }
    }
}

// ── CloseReason ───────────────────────────────────────────────────────────────

/// Freedesktop spec close reasons
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CloseReason {
    Expired     = 1,
    DismissedByUser = 2,
    ClosedByCall    = 3,
    #[allow(dead_code)]
    Undefined       = 4,
}

// ── NotificationAction ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationAction {
    pub key:   String,
    pub label: String,
}

// ── Notification ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id:             u32,
    pub app_name:       String,
    pub summary:        String,
    pub body:           String,
    pub icon:           String,
    pub urgency:        Urgency,
    pub timestamp:      i64,
    /// source_id = replaces_id from D-Bus Notify call
    pub source_id:      u32,
    pub actions:        Vec<NotificationAction>,
    /// Raw hint strings preserved for persistence (key → JSON value)
    pub hints:          HashMap<String, String>,
    pub expire_timeout: i32,
    pub state:          NotificationState,
    pub group_key:      Option<String>,
    pub can_reply:      bool,
    pub reply_placeholder: String,
    /// Maximum characters allowed in a reply (None = unlimited).
    /// Set via the `x-ember-max-reply-length` hint.
    pub max_reply_length: Option<u32>,
}

impl Notification {
    pub fn is_active(&self) -> bool {
        self.state == NotificationState::Active
    }

    pub fn effective_timeout_ms(&self, normal: u64, critical: u64, low: u64) -> Option<u64> {
        if self.expire_timeout > 0 {
            return Some(self.expire_timeout as u64);
        }
        if self.expire_timeout == 0 {
            return None; // never expire
        }
        // -1 = use default per urgency
        match self.urgency {
            Urgency::Critical => {
                if critical == 0 { None } else { Some(critical) }
            }
            Urgency::Low    => {
                if low == 0 { None } else { Some(low) }
            }
            Urgency::Normal => {
                if normal == 0 { None } else { Some(normal) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notif(urgency: Urgency, expire_timeout: i32) -> Notification {
        Notification {
            id: 1,
            app_name: "test".into(),
            summary: "s".into(),
            body: "b".into(),
            icon: "".into(),
            urgency,
            timestamp: 0,
            source_id: 0,
            actions: vec![],
            hints: HashMap::new(),
            expire_timeout,
            state: NotificationState::Active,
            group_key: None,
            can_reply: false,
            reply_placeholder: "Reply…".into(),
            max_reply_length: None,
        }
    }

    // ── Urgency ──────────────────────────────────────────────────────────────

    #[test]
    fn urgency_from_u8_roundtrips() {
        assert_eq!(Urgency::from_u8(0), Urgency::Low);
        assert_eq!(Urgency::from_u8(1), Urgency::Normal);
        assert_eq!(Urgency::from_u8(2), Urgency::Critical);
    }

    #[test]
    fn urgency_from_u8_unknown_maps_to_normal() {
        assert_eq!(Urgency::from_u8(99), Urgency::Normal);
    }

    #[test]
    fn urgency_ordering() {
        assert!(Urgency::Low < Urgency::Normal);
        assert!(Urgency::Normal < Urgency::Critical);
    }

    // ── NotificationState ─────────────────────────────────────────────────────

    #[test]
    fn notification_state_from_i32() {
        assert_eq!(NotificationState::from_i32(0), NotificationState::Active);
        assert_eq!(NotificationState::from_i32(1), NotificationState::Dismissed);
        assert_eq!(NotificationState::from_i32(2), NotificationState::Expired);
        assert_eq!(NotificationState::from_i32(99), NotificationState::Active);
    }

    // ── effective_timeout_ms ──────────────────────────────────────────────────

    #[test]
    fn explicit_positive_timeout_used_as_is() {
        let n = notif(Urgency::Normal, 5000);
        assert_eq!(n.effective_timeout_ms(3000, 0, 0), Some(5000));
    }

    #[test]
    fn expire_timeout_zero_means_never_expire() {
        let n = notif(Urgency::Normal, 0);
        assert_eq!(n.effective_timeout_ms(3000, 0, 0), None);
    }

    #[test]
    fn expire_timeout_minus_one_uses_urgency_normal() {
        let n = notif(Urgency::Normal, -1);
        assert_eq!(n.effective_timeout_ms(3000, 10000, 1000), Some(3000));
    }

    #[test]
    fn expire_timeout_minus_one_uses_urgency_critical() {
        let n = notif(Urgency::Critical, -1);
        assert_eq!(n.effective_timeout_ms(3000, 10000, 1000), Some(10000));
    }

    #[test]
    fn expire_timeout_minus_one_uses_urgency_low() {
        let n = notif(Urgency::Low, -1);
        assert_eq!(n.effective_timeout_ms(3000, 10000, 1000), Some(1000));
    }

    #[test]
    fn expire_timeout_minus_one_critical_zero_timeout_means_none() {
        let n = notif(Urgency::Critical, -1);
        assert_eq!(n.effective_timeout_ms(3000, 0, 1000), None);
    }

    // ── is_active ─────────────────────────────────────────────────────────────

    #[test]
    fn is_active_only_for_active_state() {
        assert!(notif(Urgency::Normal, -1).is_active());
        let mut n = notif(Urgency::Normal, -1);
        n.state = NotificationState::Dismissed;
        assert!(!n.is_active());
        n.state = NotificationState::Expired;
        assert!(!n.is_active());
    }
}
