use serde::{Deserialize, Serialize};
use crate::store::models::Notification;

/// Current engine state snapshot sent to UI consumers (Eww).
#[derive(Debug, Serialize, Deserialize)]
pub struct StateDto {
    pub unread:        u32,
    pub dnd:           bool,
    pub focused_id:    Option<u32>,
    pub notifications: Vec<Notification>,
}

/// Lightweight version of Notification for Eww bar/center widgets.
#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationDto {
    pub id:        u32,
    pub app_name:  String,
    pub summary:   String,
    pub body:      String,
    pub icon:      String,
    pub urgency:   u8,
    pub timestamp: i64,
    pub actions:   Vec<ActionDto>,
    pub group_key: String,
    pub can_reply: bool,
    pub reply_placeholder: String,
}


#[derive(Debug, Serialize, Deserialize)]
pub struct ActionDto {
    pub key:   String,
    pub label: String,
}

/// Group of notifications sharing the same `group_key`.
#[derive(Debug, Serialize)]
pub struct GroupDto {
    pub key:           String,
    pub count:         u32,
    pub notifications: Vec<NotificationDto>,
}

impl From<&Notification> for NotificationDto {
    fn from(n: &Notification) -> Self {
        Self {
            id:        n.id,
            app_name:  n.app_name.clone(),
            summary:   n.summary.clone(),
            body:      n.body.clone(),
            icon:      n.icon.clone(),
            urgency:   n.urgency as u8,
            timestamp: n.timestamp,
            actions:   n.actions.iter().map(|a| ActionDto {
                key:   a.key.clone(),
                label: a.label.clone(),
            }).collect(),
            group_key: n.group_key.clone().unwrap_or_default(),
            can_reply: n.can_reply,
            reply_placeholder: n.reply_placeholder.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::store::models::{Notification, NotificationAction, NotificationState, Urgency};

    fn make_notif(id: u32, urgency: Urgency) -> Notification {
        Notification {
            id,
            app_name: "TestApp".into(),
            summary: "Hello".into(),
            body: "World".into(),
            icon: "test-icon".into(),
            urgency,
            timestamp: 1_000_000,
            source_id: 0,
            actions: vec![
                NotificationAction { key: "default".into(), label: "Open".into() },
            ],
            hints: HashMap::new(),
            expire_timeout: -1,
            state: NotificationState::Active,
            group_key: Some("test-group".into()),
            can_reply: true,
            reply_placeholder: "Reply here…".into(),
            max_reply_length: None,
        }
    }

    #[test]
    fn notification_dto_from_notification_maps_fields() {
        let notif = make_notif(42, Urgency::Normal);
        let dto = NotificationDto::from(&notif);

        assert_eq!(dto.id, 42);
        assert_eq!(dto.app_name, "TestApp");
        assert_eq!(dto.summary, "Hello");
        assert_eq!(dto.body, "World");
        assert_eq!(dto.icon, "test-icon");
        assert_eq!(dto.urgency, Urgency::Normal as u8);
        assert_eq!(dto.timestamp, 1_000_000);
        assert_eq!(dto.group_key, "test-group");
        assert!(dto.can_reply);
        assert_eq!(dto.reply_placeholder, "Reply here…");
    }

    #[test]
    fn notification_dto_actions_are_mapped() {
        let notif = make_notif(1, Urgency::Normal);
        let dto = NotificationDto::from(&notif);

        assert_eq!(dto.actions.len(), 1);
        assert_eq!(dto.actions[0].key, "default");
        assert_eq!(dto.actions[0].label, "Open");
    }

    #[test]
    fn notification_dto_group_key_none_becomes_empty_string() {
        let mut notif = make_notif(1, Urgency::Low);
        notif.group_key = None;
        let dto = NotificationDto::from(&notif);
        assert_eq!(dto.group_key, "");
    }

    #[test]
    fn notification_dto_urgency_encoded_as_u8() {
        assert_eq!(NotificationDto::from(&make_notif(1, Urgency::Low)).urgency,      0);
        assert_eq!(NotificationDto::from(&make_notif(1, Urgency::Normal)).urgency,   1);
        assert_eq!(NotificationDto::from(&make_notif(1, Urgency::Critical)).urgency, 2);
    }

    #[test]
    fn notification_dto_serializes_to_json() {
        let notif = make_notif(7, Urgency::Critical);
        let dto = NotificationDto::from(&notif);
        let json = serde_json::to_string(&dto).expect("serialize");
        assert!(json.contains("\"id\":7"));
        assert!(json.contains("\"app_name\":\"TestApp\""));
        assert!(json.contains("\"urgency\":2"));
    }

    #[test]
    fn state_dto_serializes_correctly() {
        let notif = make_notif(3, Urgency::Normal);
        let dto = StateDto {
            unread: 2,
            dnd: false,
            focused_id: Some(3),
            notifications: vec![notif],
        };
        let json = serde_json::to_string(&dto).expect("serialize");
        assert!(json.contains("\"unread\":2"));
        assert!(json.contains("\"dnd\":false"));
        assert!(json.contains("\"focused_id\":3"));
    }
}
