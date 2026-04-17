//! Basic reply routing.
//! Phase 2 implementation: support reply-capable notifications flagged by hints.

pub trait ReplyRouter: Send + Sync {
    fn can_handle(&self, notif: &crate::store::models::Notification) -> bool;
    fn send_reply(&self, notif: &crate::store::models::Notification, text: &str) -> bool;
}

pub struct DefaultReplyRouter;

impl ReplyRouter for DefaultReplyRouter {
    fn can_handle(&self, notif: &crate::store::models::Notification) -> bool {
        notif.can_reply
    }

    fn send_reply(&self, notif: &crate::store::models::Notification, text: &str) -> bool {
        if !self.can_handle(notif) || text.trim().is_empty() {
            return false;
        }

        // Phase 2 baseline: log and shell out to an optional user hook.
        log::info!(
            "reply router: app='{}' id={} text='{}'",
            notif.app_name,
            notif.id,
            text.trim()
        );

        let hook = dirs::config_dir()
            .unwrap_or_default()
            .join("ember/reply-hook.sh");
        if hook.exists() {
            return std::process::Command::new("sh")
                .arg(hook)
                .arg(&notif.app_name)
                .arg(notif.id.to_string())
                .arg(text.trim())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::store::models::{Notification, NotificationState, Urgency};

    fn make_notif(can_reply: bool) -> Notification {
        Notification {
            id: 42,
            app_name: "TestApp".into(),
            summary: "Hello".into(),
            body: "World".into(),
            icon: "".into(),
            urgency: Urgency::Normal,
            timestamp: 0,
            source_id: 0,
            actions: vec![],
            hints: HashMap::new(),
            expire_timeout: -1,
            state: NotificationState::Active,
            group_key: None,
            can_reply,
            reply_placeholder: "Reply…".into(),
            max_reply_length: None,
        }
    }

    #[test]
    fn can_handle_returns_false_when_can_reply_is_false() {
        let router = DefaultReplyRouter;
        let notif = make_notif(false);
        assert!(!router.can_handle(&notif));
    }

    #[test]
    fn can_handle_returns_true_when_can_reply_is_true() {
        let router = DefaultReplyRouter;
        let notif = make_notif(true);
        assert!(router.can_handle(&notif));
    }

    #[test]
    fn send_reply_returns_false_when_can_reply_is_false() {
        let router = DefaultReplyRouter;
        let notif = make_notif(false);
        assert!(!router.send_reply(&notif, "hello"));
    }

    #[test]
    fn send_reply_returns_false_for_empty_text() {
        let router = DefaultReplyRouter;
        let notif = make_notif(true);
        assert!(!router.send_reply(&notif, ""));
    }

    #[test]
    fn send_reply_returns_false_for_whitespace_only_text() {
        let router = DefaultReplyRouter;
        let notif = make_notif(true);
        assert!(!router.send_reply(&notif, "   "));
    }

    #[test]
    fn send_reply_returns_true_when_no_hook_exists() {
        // No reply-hook.sh installed → should return true (best-effort baseline)
        let router = DefaultReplyRouter;
        let notif = make_notif(true);
        assert!(router.send_reply(&notif, "some reply text"));
    }
}
