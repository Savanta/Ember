use serde::{Deserialize, Serialize};

// ── IPC Commands ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcCommand {
    /// Current state: unread count, DND flag, active notification list.
    GetState,
    /// Dismiss a single notification by ID.
    Dismiss { id: u32 },
    /// Dismiss all active notifications.
    ClearAll,
    /// Delete all records from the history database.
    ClearHistory,
    /// Delete a single notification record from the history database.
    DeleteNotification { id: u32 },
    /// Reset the unread badge counter to zero.
    MarkAllRead,
    /// Toggle DND.
    ToggleDnd,
    /// Invoke a named action on a notification.
    InvokeAction { id: u32, action: String },
    /// Send an inline reply text to a notification when supported.
    Reply { id: u32, text: String },
    /// Paginated history query.
    History {
        #[serde(default = "default_limit")]
        limit:  usize,
        #[serde(default)]
        offset: usize,
    },
    /// Filter active notifications by app name and/or urgency.
    Filter {
        /// Only return notifications from this app (prefix match).
        #[serde(default)]
        app: Option<String>,
        /// Only return notifications at or above this urgency (0=Low, 1=Normal, 2=Critical).
        #[serde(default)]
        urgency: Option<u8>,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Get notifications grouped by app/conversation.
    GetGroups,
    /// Full-text search across history (summary, body, app_name).
    Search {
        query: String,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    /// Subscribe to state-change events.
    /// Keeps the connection open and emits a JSON line on every state change.
    Subscribe,
}

fn default_limit() -> usize {
    50
}

// ── IPC Responses ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IpcResponse {
    Ok,
    State {
        unread: u32,
        dnd:    bool,
        focused_id: Option<u32>,
        notifications: Vec<crate::ui_contract::dto::NotificationDto>,
    },
    Dnd {
        enabled: bool,
    },
    History {
        items: Vec<crate::ui_contract::dto::NotificationDto>,
    },
    Search {
        items: Vec<crate::ui_contract::dto::NotificationDto>,
    },
    Filtered {
        items: Vec<crate::ui_contract::dto::NotificationDto>,
    },
    Groups {
        items: Vec<crate::ui_contract::dto::GroupDto>,
    },
    Error {
        message: String,
    },
    /// Pushed to subscribers on every state change.
    Event {
        waiting: u32,
        history: u32,
        unread:  u32,
        dnd:     bool,
    },
}

#[cfg(test)]
mod tests {
    use super::{IpcCommand, IpcResponse};

    #[test]
    fn search_command_uses_default_limit() {
        let raw = r#"{"cmd":"search","query":"mail"}"#;
        let cmd: IpcCommand = serde_json::from_str(raw).expect("parse search cmd");

        match cmd {
            IpcCommand::Search { query, limit } => {
                assert_eq!(query, "mail");
                assert_eq!(limit, 50);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn history_command_defaults_offset_to_zero() {
        let raw = r#"{"cmd":"history","limit":10}"#;
        let cmd: IpcCommand = serde_json::from_str(raw).expect("parse history cmd");

        match cmd {
            IpcCommand::History { limit, offset } => {
                assert_eq!(limit, 10);
                assert_eq!(offset, 0);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn search_response_serializes_status_tag() {
        let rsp = IpcResponse::Search { items: vec![] };
        let json = serde_json::to_value(rsp).expect("serialize response");

        assert_eq!(json.get("status").and_then(|v| v.as_str()), Some("search"));
        assert!(json.get("items").is_some());
    }
}
