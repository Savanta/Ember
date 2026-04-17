//! Global keyboard shortcut capture using X11 `XGrabKey`.
//!
//! Runs on a dedicated thread; dispatches engine commands via a
//! `tokio::runtime::Handle` so all engine mutations stay async.

use std::collections::HashMap;
use std::sync::Arc;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ConnectionExt as _, EventMask, GrabMode, ModMask,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use crate::config::ShortcutConfig;
use crate::core::engine::Engine;
use crate::core::shortcuts::{shortcuts_from_config, ShortcutAction};
use crate::store::models::CloseReason;

/// Blocking keyboard controller loop — call from a dedicated `std::thread`.
///
/// * `engine` — shared engine handle
/// * `rt`     — handle to the tokio runtime so we can spawn async tasks
pub fn run(engine: Arc<Engine>, rt: tokio::runtime::Handle, shortcuts: ShortcutConfig) {
    let (conn, screen_num) = match RustConnection::connect(None) {
        Ok(c) => c,
        Err(e) => {
            log::error!("keyboard: X11 connect failed: {e}");
            return;
        }
    };

    let setup     = conn.setup();
    let screen    = &setup.roots[screen_num];
    let root      = screen.root;
    let min_kc    = setup.min_keycode;
    let max_kc    = setup.max_keycode;
    let n_keycodes = (max_kc - min_kc) as usize + 1;

    // Subscribe to KeyPress events on root so grabbed keys are delivered to us.
    if let Err(e) = conn.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::KEY_PRESS),
    ) {
        log::warn!("keyboard: change_window_attributes on root: {e}");
    }

    // Fetch the full keyboard mapping (keysyms_per_keycode columns × n_keycodes rows).
    let mapping_cookie = match conn.get_keyboard_mapping(min_kc, n_keycodes as u8) {
        Ok(c) => c,
        Err(e) => {
            log::error!("keyboard: get_keyboard_mapping: {e}");
            return;
        }
    };
    let mapping = match mapping_cookie.reply() {
        Ok(m) => m,
        Err(e) => {
            log::error!("keyboard: get_keyboard_mapping reply: {e}");
            return;
        }
    };

    let syms_per_kc = mapping.keysyms_per_keycode as usize;

    // Build keysym → first matching keycode map.
    let mut sym_to_kc: HashMap<u32, u8> = HashMap::new();
    for (i, chunk) in mapping.keysyms.chunks(syms_per_kc).enumerate() {
        let kc = min_kc + i as u8;
        for &sym in chunk {
            if sym != 0 {
                sym_to_kc.entry(sym).or_insert(kc);
            }
        }
    }

    // Parse configured shortcuts and grab each one on the root window.
    // Map (keycode, mods_u16) → ShortcutAction for event matching.
    let mut grabs: HashMap<(u8, u16), ShortcutAction> = HashMap::new();

    for s in &shortcuts_from_config(&shortcuts) {
        let (mods, sym) = match parse_binding(&s.binding) {
            Some(v) => v,
            None => {
                log::warn!("keyboard: cannot parse binding '{}'", s.binding);
                continue;
            }
        };
        let kc = match sym_to_kc.get(&sym) {
            Some(&k) => k,
            None => {
                log::warn!(
                    "keyboard: keysym {:#010x} not found for binding '{}'",
                    sym, s.binding
                );
                continue;
            }
        };

        match conn.grab_key(false, root, mods, kc, GrabMode::ASYNC, GrabMode::ASYNC) {
            Ok(cookie) => {
                cookie.check().ok(); // discard void cookie error
            }
            Err(e) => {
                log::warn!("keyboard: grab_key failed for '{}': {e}", s.binding);
                continue;
            }
        }

        grabs.insert((kc, u16::from(mods)), s.action.clone());
        log::debug!(
            "keyboard: grabbed '{}' → {:?} (kc={}, mods={:#06x})",
            s.binding, s.action, kc, u16::from(mods)
        );
    }

    conn.flush().ok();
    log::info!("keyboard controller running, {} shortcuts active", grabs.len());

    // Mask for modifier bits we care about (ignore NumLock / CapsLock / etc.).
    const RELEVANT_MODS: u16 =
        (1 << 0) |  // SHIFT
        (1 << 2) |  // CONTROL
        (1 << 3) |  // M1 (Alt)
        (1 << 6);   // M4 (Super)

    loop {
        let event = match conn.wait_for_event() {
            Ok(e)  => e,
            Err(e) => {
                log::error!("keyboard: wait_for_event: {e}");
                break;
            }
        };

        let Event::KeyPress(kp) = event else { continue };

        let state_bits = u16::from(kp.state) & RELEVANT_MODS;
        let key = (kp.detail, state_bits);

        if let Some(action) = grabs.get(&key) {
            let eng = Arc::clone(&engine);
            let a   = action.clone();
            rt.spawn(async move { dispatch(eng, a).await });
        }
    }
}

// ── Binding parser ────────────────────────────────────────────────────────────

/// Parse `"ctrl+shift+Delete"` → `(ModMask, keysym_u32)`.
fn parse_binding(binding: &str) -> Option<(ModMask, u32)> {
    let parts: Vec<&str> = binding.split('+').collect();
    if parts.is_empty() {
        return None;
    }
    let key_name = *parts.last()?;
    let mut mods = ModMask::from(0u16);

    for &part in &parts[..parts.len() - 1] {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => mods = mods | ModMask::CONTROL,
            "shift"            => mods = mods | ModMask::SHIFT,
            "alt"              => mods = mods | ModMask::M1,
            "super" | "mod4"   => mods = mods | ModMask::M4,
            other => {
                log::warn!("keyboard: unknown modifier '{}'", other);
                return None;
            }
        }
    }

    let sym = keysym_from_name(key_name)?;
    Some((mods, sym))
}

/// Minimal keysym-name → keysym-value table covering the default shortcuts
/// plus common additional keys.
fn keysym_from_name(name: &str) -> Option<u32> {
    let sym = match name {
        "grave"        => 0x0060,
        "asciitilde"   => 0x007e,
        "Delete"       => 0xffff,
        "Return"       => 0xff0d,
        "KP_Enter"     => 0xff8d,
        "BackSpace"    => 0xff08,
        "Escape"       => 0xff1b,
        "space"        => 0x0020,
        "Tab"          => 0xff09,
        "Up"           => 0xff52,
        "Down"         => 0xff54,
        "Left"         => 0xff51,
        "Right"        => 0xff53,
        "Home"         => 0xff50,
        "End"          => 0xff57,
        "Page_Up"      => 0xff55,
        "Page_Down"    => 0xff56,
        "Insert"       => 0xff63,
        "F1"           => 0xffbe, "F2"  => 0xffbf, "F3"  => 0xffc0,
        "F4"           => 0xffc1, "F5"  => 0xffc2, "F6"  => 0xffc3,
        "F7"           => 0xffc4, "F8"  => 0xffc5, "F9"  => 0xffc6,
        "F10"          => 0xffc7, "F11" => 0xffc8, "F12" => 0xffc9,
        other => {
            // Single ASCII character → keysym equals the ASCII code point.
            let c = other.chars().next()?;
            if other.chars().count() == 1 && c.is_ascii() {
                c as u32
            } else {
                return None;
            }
        }
    };
    Some(sym)
}

// ── Action dispatcher ─────────────────────────────────────────────────────────

async fn dispatch(engine: Arc<Engine>, action: ShortcutAction) {
    match action {
        ShortcutAction::ToggleDnd => {
            engine.toggle_dnd().await;
        }

        ShortcutAction::ClearAll => {
            engine.dismiss_all().await;
        }

        ShortcutAction::DismissFocused => {
            if let Some(notif) = engine.get_focused_notification().await {
                engine.dismiss(notif.id, CloseReason::DismissedByUser).await;
            }
        }

        ShortcutAction::InvokeDefault => {
            if let Some(notif) = engine.get_focused_notification().await {
                engine.invoke_action(notif.id, "default").await;
            }
        }

        ShortcutAction::InvokeAction(slot) => {
            if let Some(notif) = engine.get_focused_notification().await
                && let Some(act) = notif.actions.get(slot as usize) {
                    let key = act.key.clone();
                    engine.invoke_action(notif.id, &key).await;
                }
        }

        ShortcutAction::FocusNext => {
            let _ = engine.focus_next().await;
        }

        ShortcutAction::FocusPrev => {
            let _ = engine.focus_prev().await;
        }

        ShortcutAction::OpenCenter => {
            // Toggle the eww notifications popup via popup-toggle.sh.
            let _ = std::process::Command::new("sh")
                .args(["-c", "~/.config/eww/scripts/popup-toggle.sh notifications 420 &"])
                .spawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── keysym_from_name ─────────────────────────────────────────────────────

    #[test]
    fn keysym_special_keys() {
        assert_eq!(keysym_from_name("Delete"),    Some(0xffff));
        assert_eq!(keysym_from_name("Return"),    Some(0xff0d));
        assert_eq!(keysym_from_name("BackSpace"), Some(0xff08));
        assert_eq!(keysym_from_name("Escape"),    Some(0xff1b));
        assert_eq!(keysym_from_name("grave"),     Some(0x0060));
        assert_eq!(keysym_from_name("Tab"),       Some(0xff09));
    }

    #[test]
    fn keysym_arrow_keys() {
        assert_eq!(keysym_from_name("Up"),    Some(0xff52));
        assert_eq!(keysym_from_name("Down"),  Some(0xff54));
        assert_eq!(keysym_from_name("Left"),  Some(0xff51));
        assert_eq!(keysym_from_name("Right"), Some(0xff53));
    }

    #[test]
    fn keysym_single_ascii_char() {
        assert_eq!(keysym_from_name("n"), Some(b'n' as u32));
        assert_eq!(keysym_from_name("1"), Some(b'1' as u32));
        assert_eq!(keysym_from_name("a"), Some(b'a' as u32));
    }

    #[test]
    fn keysym_unknown_returns_none() {
        assert_eq!(keysym_from_name("XYZ_UNKNOWN_KEY"), None);
        assert_eq!(keysym_from_name(""),                None);
    }

    #[test]
    fn keysym_function_keys() {
        assert_eq!(keysym_from_name("F1"),  Some(0xffbe));
        assert_eq!(keysym_from_name("F12"), Some(0xffc9));
    }

    // ── parse_binding ────────────────────────────────────────────────────────

    #[test]
    fn parse_binding_ctrl_plus_key() {
        let result = parse_binding("ctrl+grave");
        assert!(result.is_some());
        let (mods, sym) = result.unwrap();
        assert_eq!(sym, 0x0060);
        // ModMask::CONTROL bit
        let mods_u16: u16 = mods.into();
        assert_ne!(mods_u16 & 0x0004, 0); // ControlMask = 4
    }

    #[test]
    fn parse_binding_ctrl_shift_delete() {
        let result = parse_binding("ctrl+shift+Delete");
        assert!(result.is_some());
        let (mods, sym) = result.unwrap();
        assert_eq!(sym, 0xffff);
        let mods_u16: u16 = mods.into();
        assert_ne!(mods_u16 & 0x0004, 0); // ctrl
        assert_ne!(mods_u16 & 0x0001, 0); // shift
    }

    #[test]
    fn parse_binding_super_modifier() {
        let result = parse_binding("super+n");
        assert!(result.is_some());
        let (mods, sym) = result.unwrap();
        assert_eq!(sym, b'n' as u32);
        let mods_u16: u16 = mods.into();
        assert_ne!(mods_u16 & 0x0040, 0); // Mod4Mask = 64
    }

    #[test]
    fn parse_binding_unknown_modifier_returns_none() {
        assert!(parse_binding("hyper+Delete").is_none());
    }

    #[test]
    fn parse_binding_unknown_key_returns_none() {
        assert!(parse_binding("ctrl+UnknownKey").is_none());
    }

    #[test]
    fn parse_binding_no_modifier_single_key() {
        let result = parse_binding("Return");
        assert!(result.is_some());
        let (mods, sym) = result.unwrap();
        assert_eq!(sym, 0xff0d);
        let mods_u16: u16 = mods.into();
        assert_eq!(mods_u16, 0); // no modifiers
    }
}
