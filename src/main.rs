mod config;
mod core;
mod ctl;
mod dbus;
mod input;
mod ipc;
mod sound;
mod store;
mod toast;
mod ui_contract;

use std::sync::Arc;
use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::core::engine::{DbusSignal, Engine};
use crate::dbus::server::NotificationsServer;
use crate::sound::SoundPlayer;
use crate::store::sqlite::SqliteStore;
use crate::toast::{ToastCommand, ToastEvent};

// ── CLI ────────────────────────────────────────────────────────────────────────

enum CliMode {
    Daemon { config_path: Option<std::path::PathBuf>, check_config: bool },
    Ctl    { config_path: Option<std::path::PathBuf>, args: Vec<String> },
    Install,
}

fn parse_args() -> Result<CliMode> {
    let mut raw = std::env::args().skip(1).peekable();

    // Detect subcommands as first positional argument.
    match raw.peek().map(|s| s.as_str()) {
        Some("ctl") => {
            raw.next(); // consume "ctl"
            // Allow `ember ctl --config <path> <cmd> …`
            let mut config_path: Option<std::path::PathBuf> = None;
            let mut rest: Vec<String> = vec![];
            let mut args_iter = raw.peekable();
            while let Some(a) = args_iter.next() {
                match a.as_str() {
                    "-c" | "--config" => {
                        let p = args_iter.next().ok_or_else(|| anyhow::anyhow!("--config requires a path"))?;
                        config_path = Some(std::path::PathBuf::from(p));
                    }
                    other if other.starts_with("--config=") => {
                        config_path = Some(std::path::PathBuf::from(&other["--config=".len()..]));
                    }
                    _ => rest.push(a),
                }
            }
            return Ok(CliMode::Ctl { config_path, args: rest });
        }
        Some("install") => return Ok(CliMode::Install),
        _ => {}
    }

    // Daemon mode — parse flags.
    let mut config_path  = None;
    let mut check_config = false;

    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!(
                    "ember {}\n\
                     Notification daemon for i3/X11\n\
                     \n\
                     USAGE:\n  ember [OPTIONS | SUBCOMMAND]\n\
                     \n\
                     OPTIONS:\n\
                       -h, --help              Print this help message\n\
                       -V, --version           Print version and exit\n\
                       -c, --config <PATH>     Use the specified config file\n\
                           --check-config      Validate config and exit\n\
                     \n\
                     SUBCOMMANDS:\n\
                       ctl <CMD>               Control the running daemon (see ember ctl --help)\n\
                       install                 Install systemd user service\n\
                     \n\
                     ENVIRONMENT:\n\
                       RUST_LOG                Log filter (default: ember=info)",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("ember {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--check-config" => {
                check_config = true;
            }
            "-c" | "--config" => {
                let path = raw.next().ok_or_else(|| anyhow::anyhow!("--config requires a path argument"))?;
                config_path = Some(std::path::PathBuf::from(path));
            }
            other if other.starts_with("--config=") => {
                config_path = Some(std::path::PathBuf::from(&other["--config=".len()..]));
            }
            unknown => {
                eprintln!("ember: unknown option: {unknown}");
                eprintln!("Run `ember --help` for usage.");
                std::process::exit(1);
            }
        }
    }
    Ok(CliMode::Daemon { config_path, check_config })
}

#[tokio::main]
async fn main() -> Result<()> {
    let mode = parse_args()?;

    // ── Logging ────────────────────────────────────────────────────────────────
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("ember=info"))
        .init();

    match mode {
        // ── ember ctl ─────────────────────────────────────────────────────────
        CliMode::Ctl { config_path, args } => {
            let cfg = match config_path {
                Some(ref p) => Config::load_from(p)?,
                None        => Config::load()?,
            };
            return ctl::run_ctl(args.into_iter(), &cfg.socket_path());
        }

        // ── ember install ─────────────────────────────────────────────────────
        CliMode::Install => {
            return install_systemd_service();
        }

        CliMode::Daemon { config_path, check_config } => {
            run_daemon(config_path, check_config).await?;
        }
    }
    Ok(())
}

fn install_systemd_service() -> Result<()> {
    let exe = std::env::current_exe().context("cannot determine current executable path")?;
    let systemd_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("cannot determine $XDG_CONFIG_HOME"))?
        .join("systemd/user");
    std::fs::create_dir_all(&systemd_dir)?;
    let service_path = systemd_dir.join("ember.service");

    let unit = format!(
        "[Unit]\n\
         Description=Ember notification daemon\n\
         Documentation=man:ember(1)\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe}\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        exe = exe.display()
    );

    std::fs::write(&service_path, &unit)?;
    println!("Wrote {}", service_path.display());
    println!();
    println!("To enable and start:");
    println!("  systemctl --user daemon-reload");
    println!("  systemctl --user enable --now ember.service");
    Ok(())
}

async fn run_daemon(
    config_path: Option<std::path::PathBuf>,
    check_config: bool,
) -> Result<()> {
    // ── Config ─────────────────────────────────────────────────────────────────
    let cfg = Arc::new(match config_path {
        Some(ref p) => Config::load_from(p)?,
        None        => Config::load()?,
    });

    if check_config {
        println!("ember: config OK");
        println!("  socket : {}", cfg.socket_path().display());
        println!("  db     : {}", cfg.db_path().display());
        println!("  dnd    : {}", cfg.dnd.enabled);
        println!("  toasts : max {} at {}", cfg.toast.max_visible, cfg.toast.position);
        std::process::exit(0);
    }

    log::info!("config loaded, socket={}", cfg.socket_path().display());

    // ── SQLite ─────────────────────────────────────────────────────────────────
    let sqlite = SqliteStore::open(&cfg.db_path()).await?;

    // ── Channels ───────────────────────────────────────────────────────────────
    let (toast_tx,       toast_rx)       = mpsc::channel::<ToastCommand>(64);
    let (toast_event_tx, mut toast_event_rx) = mpsc::channel::<ToastEvent>(64);
    let (dbus_signal_tx, dbus_signal_rx) = mpsc::channel::<DbusSignal>(64);

    // ── Engine ─────────────────────────────────────────────────────────────────
    let sound = if cfg.sound.enabled {
        SoundPlayer::new().map(std::sync::Arc::new)
    } else {
        None
    };
    let engine = Arc::new(Engine::new(
        Arc::clone(&cfg),
        sqlite,
        toast_tx,
        dbus_signal_tx,
        sound,
    ));

    // ── Toast renderer (blocking X11 thread) ──────────────────────────────────
    let toast_cfg = cfg.toast.clone();
    std::thread::Builder::new()
        .name("ember-toast".into())
        .spawn(move || {
            toast::renderer::run(toast_rx, toast_event_tx, toast_cfg);
        })?;

    // ── Keyboard shortcut controller (blocking X11 thread) ────────────────────
    {
        let eng       = Arc::clone(&engine);
        let handle    = tokio::runtime::Handle::current();
        let shortcuts = cfg.shortcuts.clone();
        std::thread::Builder::new()
            .name("ember-kbd".into())
            .spawn(move || {
                input::keyboard::run(eng, handle, shortcuts);
            })?;
    }

    // ── Toast event handler ────────────────────────────────────────────────────
    {
        let eng = Arc::clone(&engine);
        tokio::spawn(async move {
            while let Some(event) = toast_event_rx.recv().await {
                match event {
                    ToastEvent::Dismissed(id)                  => eng.on_toast_dismissed(id).await,
                    ToastEvent::ActionInvoked { id, key }      => eng.on_toast_action(id, key).await,
                    ToastEvent::Expired(id)                    => eng.on_toast_expired(id).await,
                    ToastEvent::ReplySubmitted { id, text }    => { eng.send_reply(id, &text).await; }
                }
            }
        });
    }

    // ── D-Bus server ───────────────────────────────────────────────────────────
    let dbus_conn = zbus::connection::Builder::session()?
        .name("org.freedesktop.Notifications")?
        .serve_at(
            "/org/freedesktop/Notifications",
            NotificationsServer::new(Arc::clone(&engine)),
        )?
        .build()
        .await?;

    log::info!("D-Bus service registered as org.freedesktop.Notifications");

    // Spawn the D-Bus signal emitter: engine events → D-Bus signals
    {
        let iface_ref = dbus_conn
            .object_server()
            .interface::<_, NotificationsServer>("/org/freedesktop/Notifications")
            .await?;

        let mut rx = dbus_signal_rx;
        tokio::spawn(async move {
            while let Some(signal) = rx.recv().await {
                let emitter = iface_ref.signal_emitter();
                match signal {
                    DbusSignal::NotificationClosed { id, reason } => {
                        let _ =
                            NotificationsServer::notification_closed(emitter, id, reason as u32)
                                .await;
                    }
                    DbusSignal::ActionInvoked { id, key } => {
                        let _ =
                            NotificationsServer::action_invoked(emitter, id, key).await;
                    }
                }
            }
        });
    }

    // ── IPC socket ────────────────────────────────────────────────────────────
    {
        let socket_path = cfg.socket_path();
        let eng         = Arc::clone(&engine);
        tokio::spawn(async move {
            ipc::socket::run(eng, &socket_path).await;
        });
    }

    // ── Config hot-reload (poll every 5 s) ────────────────────────────────────
    {
        let watched = config_path.or_else(|| {
            dirs::config_dir()
                .map(|d| d.join("ember/config.toml"))
                .filter(|p| p.exists())
        });
        if let Some(path) = watched {
            let eng = Arc::clone(&engine);
            tokio::spawn(async move {
                let mut last_mtime = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok());
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    if let Ok(meta) = std::fs::metadata(&path) {
                        if let Ok(mtime) = meta.modified() {
                            if Some(mtime) != last_mtime {
                                last_mtime = Some(mtime);
                                match Config::load_from(&path) {
                                    Ok(new_cfg) => eng.reload_config(new_cfg).await,
                                    Err(e) => log::warn!("config reload failed: {e}"),
                                }
                            }
                        }
                    }
                }
            });
        }
    }

    log::info!("Ember started — waiting for notifications");

    // ── Shutdown ───────────────────────────────────────────────────────────────
    tokio::signal::ctrl_c().await?;
    log::info!("shutting down");
    Ok(())
}
