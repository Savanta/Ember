//! `ember ctl` — thin CLI client that connects to the running daemon via IPC socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::Result;
use serde_json::json;

pub fn run_ctl(mut args: impl Iterator<Item = String>, socket_path: &Path) -> Result<()> {
    let cmd = args.next();
    let payload = match cmd.as_deref() {
        // ── Info ──────────────────────────────────────────────────────────────
        Some("state") => json!({"cmd": "get_state"}),
        Some("groups") => json!({"cmd": "get_groups"}),

        // ── Dismiss / clear ───────────────────────────────────────────────────
        Some("dismiss") => {
            let id = parse_id(args.next().as_deref(), "dismiss")?;
            json!({"cmd": "dismiss", "id": id})
        }
        Some("clear") => json!({"cmd": "clear_all"}),
        Some("delete") => {
            let id = parse_id(args.next().as_deref(), "delete")?;
            json!({"cmd": "delete_notification", "id": id})
        }

        // ── History ───────────────────────────────────────────────────────────
        Some("history") => {
            let (limit, offset) = parse_limit_offset(args)?;
            json!({"cmd": "history", "limit": limit, "offset": offset})
        }
        Some("clear-history") => json!({"cmd": "clear_history"}),

        // ── Search ────────────────────────────────────────────────────────────
        Some("search") => {
            let words: Vec<String> = args.collect();
            if words.is_empty() {
                anyhow::bail!("ember ctl search: missing query");
            }
            json!({"cmd": "search", "query": words.join(" ")})
        }

        // ── DND / unread ──────────────────────────────────────────────────────
        Some("dnd") => json!({"cmd": "toggle_dnd"}),
        Some("mark-read") => json!({"cmd": "mark_all_read"}),

        // ── Reply ─────────────────────────────────────────────────────────────
        Some("reply") => {
            let id = parse_id(args.next().as_deref(), "reply")?;
            let words: Vec<String> = args.collect();
            if words.is_empty() {
                anyhow::bail!("ember ctl reply: missing reply text");
            }
            json!({"cmd": "reply", "id": id, "text": words.join(" ")})
        }

        // ── Subscribe ─────────────────────────────────────────────────────────
        Some("subscribe") => {
            return run_subscribe(socket_path);
        }

        Some("--help") | Some("-h") | None => {
            print_help();
            return Ok(());
        }

        Some(other) => {
            eprintln!("ember ctl: unknown command '{other}' — try `ember ctl --help`");
            std::process::exit(1);
        }
    };

    let line = serde_json::to_string(&payload)?;
    let mut stream = connect(socket_path)?;
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;

    // Read exactly one response line.
    let reader = BufReader::new(&stream);
    for raw in reader.lines() {
        let raw = raw?;
        print_pretty(&raw);
        break;
    }
    Ok(())
}

/// `ember ctl subscribe` — keep connection open, print event lines until SIGINT.
fn run_subscribe(socket_path: &Path) -> Result<()> {
    let mut stream = connect(socket_path)?;
    stream.write_all(b"{\"cmd\":\"subscribe\"}\n")?;
    let reader = BufReader::new(&stream);
    for raw in reader.lines() {
        let raw = raw?;
        print_pretty(&raw);
    }
    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn connect(socket_path: &Path) -> Result<UnixStream> {
    UnixStream::connect(socket_path).map_err(|e| {
        anyhow::anyhow!(
            "cannot connect to {}: {e}\nIs ember running?",
            socket_path.display()
        )
    })
}

fn parse_id(s: Option<&str>, cmd: &str) -> Result<u32> {
    let s = s.ok_or_else(|| anyhow::anyhow!("ember ctl {cmd}: missing notification ID"))?;
    s.parse::<u32>()
        .map_err(|_| anyhow::anyhow!("ember ctl {cmd}: invalid ID '{s}' (must be a positive integer)"))
}

fn parse_limit_offset(mut args: impl Iterator<Item = String>) -> Result<(usize, usize)> {
    let mut limit  = 20usize;
    let mut offset = 0usize;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--limit" | "-n" => {
                let v = args.next().ok_or_else(|| anyhow::anyhow!("--limit requires a value"))?;
                limit = v.parse().map_err(|_| anyhow::anyhow!("--limit: expected a number, got '{v}'"))?;
            }
            "--offset" => {
                let v = args.next().ok_or_else(|| anyhow::anyhow!("--offset requires a value"))?;
                offset = v.parse().map_err(|_| anyhow::anyhow!("--offset: expected a number, got '{v}'"))?;
            }
            _ => {} // ignore unknown flags for forward-compat
        }
    }
    Ok((limit, offset))
}

fn print_pretty(raw: &str) {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(val) => println!("{}", serde_json::to_string_pretty(&val).unwrap_or_else(|_| raw.to_owned())),
        Err(_)  => println!("{raw}"),
    }
}

fn print_help() {
    println!(
        "ember ctl — control a running ember daemon\n\
         \n\
         USAGE:\n  ember ctl <COMMAND> [ARGS]\n\
         \n\
         COMMANDS:\n\
           state                     Show active notifications and DND status\n\
           groups                    Show active notifications grouped by app\n\
           dismiss <id>              Dismiss a notification by ID\n\
           clear                     Dismiss all active notifications\n\
           delete <id>               Remove a notification record from history\n\
           history [-n <N>]          Show notification history (default: 20)\n\
           clear-history             Delete all history records\n\
           search <query>            Full-text search across history\n\
           dnd                       Toggle Do Not Disturb\n\
           mark-read                 Reset unread counter to zero\n\
           reply <id> <text>         Send an inline reply to a notification\n\
           subscribe                 Stream live state events (Ctrl-C to stop)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_id_accepts_valid() {
        assert_eq!(parse_id(Some("42"), "test").unwrap(), 42);
    }

    #[test]
    fn parse_id_rejects_non_numeric() {
        assert!(parse_id(Some("abc"), "test").is_err());
    }

    #[test]
    fn parse_id_requires_value() {
        assert!(parse_id(None, "test").is_err());
    }

    #[test]
    fn parse_limit_offset_defaults() {
        let (l, o) = parse_limit_offset(std::iter::empty()).unwrap();
        assert_eq!(l, 20);
        assert_eq!(o, 0);
    }

    #[test]
    fn parse_limit_offset_explicit() {
        let args = vec!["--limit".to_owned(), "10".to_owned(), "--offset".to_owned(), "5".to_owned()];
        let (l, o) = parse_limit_offset(args.into_iter()).unwrap();
        assert_eq!(l, 10);
        assert_eq!(o, 5);
    }
}
