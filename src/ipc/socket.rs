use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use crate::core::engine::Engine;
use crate::store::models::CloseReason;
use crate::ui_contract::dto::NotificationDto;
use super::api::{IpcCommand, IpcResponse};

/// Start the Unix socket IPC server.
/// Accepts line-delimited JSON commands and responds with line-delimited JSON.
pub async fn run(engine: Arc<Engine>, path: &Path) {
    // Remove stale socket from a previous run.
    let _ = std::fs::remove_file(path);

    let listener = match UnixListener::bind(path) {
        Ok(l) => l,
        Err(e) => {
            log::error!("failed to bind IPC socket {}: {e}", path.display());
            return;
        }
    };

    log::debug!("IPC socket listening on {}", path.display());

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let eng = Arc::clone(&engine);
                tokio::spawn(async move {
                    handle_connection(stream, eng).await;
                });
            }
            Err(e) => {
                log::error!("IPC accept error: {e}");
            }
        }
    }
}

async fn handle_connection(stream: tokio::net::UnixStream, engine: Arc<Engine>) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }

        let cmd = match serde_json::from_str::<IpcCommand>(&line) {
            Err(e) => {
                let err = IpcResponse::Error { message: format!("parse error: {e}") };
                let _ = send_json(&mut writer, &err).await;
                continue;
            }
            Ok(cmd) => cmd,
        };

        // Subscribe keeps the connection open and streams events.
        if matches!(cmd, IpcCommand::Subscribe) {
            handle_subscribe(writer, engine).await;
            return;
        }

        let response = dispatch(cmd, &engine).await;
        if send_json(&mut writer, &response).await.is_err() {
            break;
        }
    }
}

/// Write a single JSON line to the writer. Returns Err if the client disconnected.
async fn send_json(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    resp: &IpcResponse,
) -> std::io::Result<()> {
    let mut json = serde_json::to_string(resp).unwrap_or_else(|_| r#"{"status":"error","message":"serialisation failed"}"#.to_owned());
    json.push('\n');
    writer.write_all(json.as_bytes()).await
}

/// Holds the connection open, emitting an Event JSON line on every state change.
async fn handle_subscribe(
    mut writer: tokio::net::unix::OwnedWriteHalf,
    engine: Arc<Engine>,
) {
    let mut rx = engine.subscribe();

    // Send the current state immediately so the client doesn't have to wait.
    let snap = engine.event_snapshot().await;
    let initial = IpcResponse::Event {
        waiting: snap.waiting,
        history: snap.history,
        unread:  snap.unread,
        dnd:     snap.dnd,
    };
    if send_json(&mut writer, &initial).await.is_err() {
        return;
    }

    loop {
        use tokio::sync::broadcast::error::RecvError;
        match rx.recv().await {
            Ok(()) | Err(RecvError::Lagged(_)) => {
                let snap = engine.event_snapshot().await;
                let ev = IpcResponse::Event {
                    waiting: snap.waiting,
                    history: snap.history,
                    unread:  snap.unread,
                    dnd:     snap.dnd,
                };
                if send_json(&mut writer, &ev).await.is_err() {
                    break; // client disconnected
                }
            }
            Err(RecvError::Closed) => break,
        }
    }
}

async fn dispatch(cmd: IpcCommand, engine: &Engine) -> IpcResponse {
    match cmd {
        IpcCommand::GetState => {
            let state = engine.get_state().await;
            let notifications = state
                .notifications
                .iter()
                .map(NotificationDto::from)
                .collect();
            IpcResponse::State {
                unread: state.unread,
                dnd:    state.dnd,
                focused_id: state.focused_id,
                notifications,
            }
        }

        IpcCommand::Dismiss { id } => {
            engine.dismiss(id, CloseReason::DismissedByUser).await;
            IpcResponse::Ok
        }

        IpcCommand::ClearAll => {
            engine.dismiss_all().await;
            IpcResponse::Ok
        }

        IpcCommand::ClearHistory => {
            engine.clear_history().await;
            IpcResponse::Ok
        }

        IpcCommand::DeleteNotification { id } => {
            engine.delete_notification(id).await;
            IpcResponse::Ok
        }

        IpcCommand::MarkAllRead => {
            engine.mark_all_read().await;
            IpcResponse::Ok
        }

        IpcCommand::ToggleDnd => {
            let enabled = engine.toggle_dnd().await;
            IpcResponse::Dnd { enabled }
        }

        IpcCommand::InvokeAction { id, action } => {
            engine.invoke_action(id, &action).await;
            IpcResponse::Ok
        }

        IpcCommand::Reply { id, text } => {
            if engine.send_reply(id, &text).await {
                IpcResponse::Ok
            } else {
                IpcResponse::Error { message: "reply not supported".into() }
            }
        }

        IpcCommand::History { limit, offset } => {
            let items = engine.history(limit, offset).await;
            IpcResponse::History {
                items: items.iter().map(NotificationDto::from).collect(),
            }
        }

        IpcCommand::Filter { app, urgency, limit } => {
            let state = engine.get_state().await;
            let items: Vec<NotificationDto> = state
                .notifications
                .iter()
                .filter(|n| {
                    app.as_deref()
                        .map(|prefix| n.app_name.to_lowercase().starts_with(&prefix.to_lowercase()))
                        .unwrap_or(true)
                })
                .filter(|n| {
                    urgency
                        .map(|min_u| n.urgency as u8 >= min_u)
                        .unwrap_or(true)
                })
                .take(limit)
                .map(NotificationDto::from)
                .collect();
            IpcResponse::Filtered { items }
        }

        IpcCommand::GetGroups => {
            let groups = engine.get_groups().await;
            IpcResponse::Groups { items: groups }
        }

        IpcCommand::Search { query, limit } => {
            let items = engine.search(&query, limit).await;
            IpcResponse::Search {
                items: items.iter().map(NotificationDto::from).collect(),
            }
        }

        // Subscribe is handled before dispatch() is called; this branch is unreachable.
        IpcCommand::Subscribe => IpcResponse::Error { message: "unreachable".into() },
    }
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    use crate::config::Config;
    use crate::core::engine::{DbusSignal, Engine, IncomingNotification};
    use crate::store::sqlite::SqliteStore;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Spin up a test Engine + IPC socket, return the socket path.
    async fn start_server() -> (Arc<Engine>, std::path::PathBuf) {
        let n   = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();

        let db_path = format!("/tmp/ember-ipc-test-{pid}-{n}.db");
        let sock_path = std::path::PathBuf::from(
            format!("/tmp/ember-ipc-test-{pid}-{n}.sock")
        );

        let sqlite = SqliteStore::open(std::path::Path::new(&db_path))
            .await
            .expect("open test db");
        let cfg: Config = toml::from_str("").expect("default config");

        let (toast_tx, _toast_rx) = tokio::sync::mpsc::channel::<crate::toast::ToastCommand>(8);
        let (dbus_tx,  _dbus_rx)  = tokio::sync::mpsc::channel::<DbusSignal>(8);

        let engine = Arc::new(Engine::new(Arc::new(cfg), sqlite, toast_tx, dbus_tx));

        let eng_clone = Arc::clone(&engine);
        let path_clone = sock_path.clone();
        tokio::spawn(async move {
            run(eng_clone, &path_clone).await;
        });

        // Give the socket a moment to bind.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        (engine, sock_path)
    }

    async fn send_cmd(path: &std::path::Path, cmd: &str) -> String {
        let mut stream = UnixStream::connect(path).await.expect("connect");
        let (reader_half, mut writer) = stream.split();
        writer.write_all(format!("{cmd}\n").as_bytes()).await.unwrap();
        let mut lines = BufReader::new(reader_half).lines();
        lines.next_line().await.unwrap().unwrap_or_default()
    }

    #[tokio::test]
    async fn get_state_returns_state_status() {
        let (_eng, sock) = start_server().await;
        let resp = send_cmd(&sock, r#"{"cmd":"get_state"}"#).await;
        let v: serde_json::Value = serde_json::from_str(&resp).expect("valid json");
        assert_eq!(v["status"], "state");
        assert_eq!(v["unread"], 0);
        assert_eq!(v["dnd"], false);
    }

    #[tokio::test]
    async fn toggle_dnd_returns_dnd_enabled() {
        let (_eng, sock) = start_server().await;
        let resp = send_cmd(&sock, r#"{"cmd":"toggle_dnd"}"#).await;
        let v: serde_json::Value = serde_json::from_str(&resp).expect("valid json");
        assert_eq!(v["status"], "dnd");
        assert_eq!(v["enabled"], true);
    }

    #[tokio::test]
    async fn toggle_dnd_twice_returns_false() {
        let (_eng, sock) = start_server().await;
        send_cmd(&sock, r#"{"cmd":"toggle_dnd"}"#).await;
        let resp = send_cmd(&sock, r#"{"cmd":"toggle_dnd"}"#).await;
        let v: serde_json::Value = serde_json::from_str(&resp).expect("valid json");
        assert_eq!(v["enabled"], false);
    }

    #[tokio::test]
    async fn clear_all_returns_ok() {
        let (_eng, sock) = start_server().await;
        let resp = send_cmd(&sock, r#"{"cmd":"clear_all"}"#).await;
        let v: serde_json::Value = serde_json::from_str(&resp).expect("valid json");
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn mark_all_read_returns_ok() {
        let (_eng, sock) = start_server().await;
        let resp = send_cmd(&sock, r#"{"cmd":"mark_all_read"}"#).await;
        let v: serde_json::Value = serde_json::from_str(&resp).expect("valid json");
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn invalid_json_returns_error() {
        let (_eng, sock) = start_server().await;
        let resp = send_cmd(&sock, r#"not json at all"#).await;
        let v: serde_json::Value = serde_json::from_str(&resp).expect("valid json");
        assert_eq!(v["status"], "error");
    }

    #[tokio::test]
    async fn get_state_after_notification_has_unread_1() {
        let (eng, sock) = start_server().await;
        eng.receive_notification(IncomingNotification {
            app_name:       "TestApp".into(),
            replaces_id:    0,
            app_icon:       "".into(),
            summary:        "Hello".into(),
            body:           "".into(),
            actions_raw:    vec![],
            hints:          std::collections::HashMap::new(),
            expire_timeout: -1,
        }).await;

        let resp = send_cmd(&sock, r#"{"cmd":"get_state"}"#).await;
        let v: serde_json::Value = serde_json::from_str(&resp).expect("valid json");
        assert_eq!(v["status"], "state");
        assert_eq!(v["unread"], 1);
    }

    #[tokio::test]
    async fn dismiss_active_notification_returns_ok() {
        let (eng, sock) = start_server().await;
        let id = eng.receive_notification(IncomingNotification {
            app_name:       "App".into(),
            replaces_id:    0,
            app_icon:       "".into(),
            summary:        "Note".into(),
            body:           "".into(),
            actions_raw:    vec![],
            hints:          std::collections::HashMap::new(),
            expire_timeout: -1,
        }).await;

        let cmd = format!(r#"{{"cmd":"dismiss","id":{id}}}"#);
        let resp = send_cmd(&sock, &cmd).await;
        let v: serde_json::Value = serde_json::from_str(&resp).expect("valid json");
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn history_returns_history_status() {
        let (_eng, sock) = start_server().await;
        let resp = send_cmd(&sock, r#"{"cmd":"history"}"#).await;
        let v: serde_json::Value = serde_json::from_str(&resp).expect("valid json");
        assert_eq!(v["status"], "history");
    }
}
