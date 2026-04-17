pub mod animation;
pub mod icon;
pub mod layout;
pub mod renderer;
pub mod theme;

use crate::store::models::Notification;

// ── Channel types shared between engine and renderer ─────────────────────────

/// Commands sent from the engine to the toast renderer thread.
#[derive(Debug)]
pub enum ToastCommand {
    Show { notif: Box<Notification>, timeout_ms: Option<u64>, group_count: u32 },
    Close(u32),
    SetFocus(Option<u32>),
    /// Update the group-stack badge on an already-visible toast.
    UpdateGroupCount { id: u32, count: u32 },
}

/// Events sent from the toast renderer thread back to the engine.
#[derive(Debug)]
pub enum ToastEvent {
    /// User clicked the close button or the window body.
    Dismissed(u32),
    /// User clicked a named action button.
    ActionInvoked { id: u32, key: String },
    /// Timeout elapsed for the notification.
    Expired(u32),
    /// User typed a reply in the inline input box and pressed Enter.
    ReplySubmitted { id: u32, text: String },
}
