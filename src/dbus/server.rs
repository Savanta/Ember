use std::collections::HashMap;
use std::sync::Arc;
use zbus::interface;
use zbus::object_server::SignalEmitter;

use crate::core::engine::{Engine, IncomingNotification};
use crate::store::models::CloseReason;

// ── Server struct ─────────────────────────────────────────────────────────────

pub struct NotificationsServer {
    engine: Arc<Engine>,
}

impl NotificationsServer {
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

// ── org.freedesktop.Notifications interface ───────────────────────────────────

#[interface(name = "org.freedesktop.Notifications")]
impl NotificationsServer {
    /// Return the set of capabilities this server supports.
    async fn get_capabilities(&self) -> Vec<String> {
        vec![
            "actions".to_string(),
            "body".to_string(),
            "body-markup".to_string(),
            "icon-static".to_string(),
            "persistence".to_string(),
        ]
    }

    /// Receive a new notification. Returns the assigned notification ID.
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        hints: HashMap<String, zbus::zvariant::OwnedValue>,
        expire_timeout: i32,
    ) -> zbus::fdo::Result<u32> {
        log::debug!("D-Bus Notify received app={} summary={} replaces_id={}", app_name, summary, replaces_id);

        let hints_str = convert_hints(hints);
        let incoming = IncomingNotification {
            app_name:       app_name.to_owned(),
            replaces_id,
            app_icon:       app_icon.to_owned(),
            summary:        summary.to_owned(),
            body:           body.to_owned(),
            actions_raw:    actions,
            hints:          hints_str,
            expire_timeout,
        };

        let id = self.engine.receive_notification(incoming).await;
        Ok(id)
    }

    /// Close a notification programmatically.
    async fn close_notification(&self, id: u32) -> zbus::fdo::Result<()> {
        self.engine.dismiss(id, CloseReason::ClosedByCall).await;
        Ok(())
    }

    /// Return server identity per the Freedesktop spec.
    async fn get_server_information(&self) -> (String, String, String, String) {
        (
            "Ember".to_string(),
            "ember-project".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            "1.2".to_string(), // spec version
        )
    }

    // ── Signals ───────────────────────────────────────────────────────────────

    /// Emitted when a notification is closed (for any reason).
    #[zbus(signal)]
    pub async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id:     u32,
        reason: u32,
    ) -> zbus::Result<()>;

    /// Emitted when a notification action is invoked by the user.
    #[zbus(signal)]
    pub async fn action_invoked(
        emitter:    &SignalEmitter<'_>,
        id:         u32,
        action_key: String,
    ) -> zbus::Result<()>;
}

// ── hint conversion ───────────────────────────────────────────────────────────

/// Convert D-Bus variant hints to plain strings for engine consumption.
/// Phase 1 only needs `urgency`; others are stored as debug strings.
fn convert_hints(
    hints: HashMap<String, zbus::zvariant::OwnedValue>,
) -> HashMap<String, String> {
    hints
        .into_iter()
        .map(|(k, v)| {
            let s = match v.downcast_ref::<u8>() {
                Ok(n) => n.to_string(),
                _ => match v.downcast_ref::<i32>() {
                    Ok(n) => n.to_string(),
                    _ => match v.downcast_ref::<bool>() {
                        Ok(b) => b.to_string(),
                        _ => match v.downcast_ref::<&str>() {
                            Ok(s) => s.to_string(),
                            _ => format!("{v:?}"),
                        },
                    },
                },
            };
            (k, s)
        })
        .collect()
}
