//! Action dispatch helpers.
//! Raise an existing window when possible; otherwise launch the application
//! using XDG desktop-entry information.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::store::models::Notification;

pub fn open_notification(notif: &Notification) {
    let mut tokens = vec![notif.app_name.clone()];

    if let Some(entry) = notif.hints.get("desktop-entry") {
        let clean = entry.trim().trim_end_matches(".desktop");
        if !clean.is_empty() {
            tokens.push(clean.to_string());
        }
    }

    if let Some(group) = &notif.group_key
        && !group.is_empty() {
            tokens.push(group.clone());
        }

    for t in normalized_tokens(&tokens) {
        if raise_window(&t) {
            log::info!("action router: raised window for '{}'", t);
            return;
        }
    }

    if let Some(entry) = find_desktop_entry(notif)
        && launch_desktop_entry(&entry) {
            log::info!("action router: launched desktop entry '{}'", entry);
            return;
        }

    for t in normalized_tokens(&tokens) {
        if Command::new("xdg-open").arg(&t).spawn().is_ok() {
            log::info!("action router: fallback xdg-open '{}'", t);
            return;
        }
    }
}

fn normalized_tokens(items: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        for s in [
            trimmed.to_string(),
            trimmed.to_lowercase(),
            trimmed.replace(' ', "-"),
            trimmed.replace(' ', "_").to_lowercase(),
        ] {
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

fn raise_window(token: &str) -> bool {
    Command::new("wmctrl")
        .args(["-xa", token])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn launch_desktop_entry(entry: &str) -> bool {
    Command::new("gtk-launch")
        .arg(entry)
        .spawn()
        .is_ok()
}

/// Dispatch a non-default action to `~/.config/ember/action-hook.sh`.
/// Arguments: `$1=app_name  $2=id  $3=action_key`
pub fn dispatch_action_hook(notif: &Notification, key: &str) {
    let hook = dirs::config_dir()
        .unwrap_or_default()
        .join("ember/action-hook.sh");
    if !hook.exists() {
        return;
    }
    let result = std::process::Command::new("sh")
        .arg(&hook)
        .arg(&notif.app_name)
        .arg(notif.id.to_string())
        .arg(key)
        .spawn();
    match result {
        Ok(_)  => log::info!("action hook dispatched: app='{}' id={} key='{}'", notif.app_name, notif.id, key),
        Err(e) => log::warn!("action hook failed to spawn: {e}"),
    }
}

fn find_desktop_entry(notif: &Notification) -> Option<String> {
    if let Some(entry) = notif.hints.get("desktop-entry") {
        let clean = entry.trim().trim_end_matches(".desktop");
        if !clean.is_empty() {
            return Some(clean.to_string());
        }
    }

    let needles = normalized_tokens(std::slice::from_ref(&notif.app_name));
    for base in xdg_data_dirs() {
        let apps = base.join("applications");
        let Ok(rd) = std::fs::read_dir(&apps) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name()?.to_string_lossy().to_lowercase();
            if needles.iter().any(|n| name == format!("{n}.desktop") || name.starts_with(&format!("{n}-")) || name.contains(n)) {
                return Some(path.file_stem()?.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut dirs_out = Vec::new();

    if let Some(home) = dirs::data_dir() {
        dirs_out.push(home);
    }

    let sys = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for p in sys.split(':') {
        if !p.is_empty() {
            let pb = Path::new(p).to_path_buf();
            if !dirs_out.contains(&pb) {
                dirs_out.push(pb);
            }
        }
    }

    dirs_out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::store::models::{NotificationState, Urgency};

    fn make_notif(app_name: &str, hints: HashMap<String, String>) -> Notification {
        Notification {
            id: 1,
            app_name: app_name.to_string(),
            summary: "test".into(),
            body: "body".into(),
            icon: "".into(),
            urgency: Urgency::Normal,
            timestamp: 0,
            source_id: 0,
            actions: vec![],
            hints,
            expire_timeout: -1,
            state: NotificationState::Active,
            group_key: None,
            can_reply: false,
            reply_placeholder: "Reply…".into(),
            max_reply_length: None,
        }
    }

    #[test]
    fn normalized_tokens_deduplicates_identical_variants() {
        // "firefox" is already lowercase, so only unique variants survive
        let tokens = normalized_tokens(&["firefox".to_string()]);
        assert!(tokens.contains(&"firefox".to_string()));
        // No duplicate entries
        let unique: std::collections::HashSet<_> = tokens.iter().collect();
        assert_eq!(tokens.len(), unique.len());
    }

    #[test]
    fn normalized_tokens_lowercases_and_replaces_spaces() {
        let tokens = normalized_tokens(&["My App".to_string()]);
        assert!(tokens.contains(&"My App".to_string()));
        assert!(tokens.contains(&"my app".to_string()));
        assert!(tokens.contains(&"My-App".to_string()));
        assert!(tokens.contains(&"my_app".to_string()));
    }

    #[test]
    fn normalized_tokens_skips_empty_items() {
        let tokens = normalized_tokens(&["".to_string(), "  ".to_string()]);
        assert!(tokens.is_empty());
    }

    #[test]
    fn find_desktop_entry_uses_hint_directly() {
        let mut hints = HashMap::new();
        hints.insert("desktop-entry".to_string(), "firefox.desktop".to_string());
        let notif = make_notif("Mozilla Firefox", hints);
        let entry = find_desktop_entry(&notif);
        // Should return "firefox" with .desktop stripped
        assert_eq!(entry.as_deref(), Some("firefox"));
    }

    #[test]
    fn find_desktop_entry_hint_without_extension() {
        let mut hints = HashMap::new();
        hints.insert("desktop-entry".to_string(), "code".to_string());
        let notif = make_notif("Visual Studio Code", hints);
        let entry = find_desktop_entry(&notif);
        assert_eq!(entry.as_deref(), Some("code"));
    }

    #[test]
    fn find_desktop_entry_empty_hint_falls_back_to_fs_search() {
        // Empty hint → should not return the empty hint, falls through to FS scan
        let mut hints = HashMap::new();
        hints.insert("desktop-entry".to_string(), "".to_string());
        let notif = make_notif("nonexistent-app-xyz", hints);
        // FS search won't find a nonexistent app — returns None without panicking
        let entry = find_desktop_entry(&notif);
        assert!(entry.is_none() || entry.as_deref() != Some(""));
    }

    #[test]
    fn xdg_data_dirs_returns_no_duplicates() {
        let dirs = xdg_data_dirs();
        let unique: std::collections::HashSet<_> = dirs.iter().collect();
        assert_eq!(dirs.len(), unique.len());
    }

    #[test]
    fn xdg_data_dirs_respects_env_override() {
        // Override XDG_DATA_DIRS to a known single path
        let tmp = std::env::temp_dir();
        let tmp_str = tmp.to_string_lossy().to_string();
        // SAFETY: test-only; no other threads are reading this env var concurrently
        unsafe { std::env::set_var("XDG_DATA_DIRS", &tmp_str); }
        let dirs = xdg_data_dirs();
        unsafe { std::env::remove_var("XDG_DATA_DIRS"); }
        assert!(dirs.contains(&tmp));
    }

    #[test]
    fn dispatch_action_hook_does_not_panic_when_hook_absent() {
        // The hook file almost certainly doesn't exist in the test environment.
        // The function must not panic — it simply logs and returns.
        let notif = make_notif("TestApp", HashMap::new());
        dispatch_action_hook(&notif, "my-action"); // must not panic
    }
}
