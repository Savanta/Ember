use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    self, AtomEnum, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask,
    ImageFormat, InputFocus, PropMode, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;   // provides change_property32
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use crate::config::{ThemeConfig, ToastConfig};
use crate::store::models::{Notification, Urgency};
use crate::toast::{ToastCommand, ToastEvent};
use crate::toast::icon::resolve_icon;
use crate::toast::layout::{position_for, ToastRect};
use crate::toast::theme::{resolve, Rgb};

// ── Active toast window ────────────────────────────────────────────────────────

/// Coordinates + dimensions of one action button (relative to the toast window).
#[derive(Clone, Copy)]
struct ButtonRect {
    x:    i16,
    y:    i16,
    w:    u16,
    h:    u16,
    /// Index into `notif.actions`.
    idx:  usize,
}

struct ToastWindow {
    win:          xproto::Window,
    gc:           xproto::Gcontext,
    notif:        Notification,
    rect:         ToastRect,
    expires_at:   Option<Instant>,
    /// Instant at which this toast was created (for progress bar fraction).
    created_at:   Instant,
    depth:        u8,
    /// True while the mouse pointer is inside this window.
    hovered:      bool,
    /// The instant we entered hover; used to extend `expires_at` when leaving.
    hover_start:  Option<Instant>,
    /// Pre-computed button hit-test rects (empty if no actions).
    button_rects: Vec<ButtonRect>,
    /// Set by restack_windows on first positioning; drives slide-in animation.
    anim_start:    Option<Instant>,
    /// Set when toast starts sliding out; window destroyed after LEAVE_SECS.
    leaving_start: Option<Instant>,
    /// Target Y when restack moves a toast to a new slot.
    target_y:      Option<i32>,
    /// When the Y-move animation started (duration = MOVE_SECS).
    move_start:    Option<Instant>,
    /// True when this toast is the keyboard-focused one.
    focused:       bool,
    /// True while the inline reply input box is active.
    reply_active:  bool,
    /// Text being typed in the inline reply input box.
    reply_text:    String,
    /// Byte-position of the text cursor inside reply_text.
    cursor_pos:    usize,
    /// Anchor byte-pos of the current text selection (None = no selection).
    /// Selection range = [min(cursor_pos,sel_anchor), max(cursor_pos,sel_anchor)).
    sel_anchor:    Option<usize>,
    /// True while the user is dragging a mouse selection inside the input.
    dragging_reply: bool,
    /// Number of active notifications in the same group (1 = no stacking badge).
    group_count:   u32,
}

// ── Renderer entry point ───────────────────────────────────────────────────────

/// Run the toast renderer. Blocks the calling thread.
/// Receives `ToastCommand`s and sends `ToastEvent`s back to the engine.
pub fn run(
    mut rx:     mpsc::Receiver<ToastCommand>,
    event_tx:   mpsc::Sender<ToastEvent>,
    mut cfg:    ToastConfig,
) {
    let (conn, screen_num) = match RustConnection::connect(None) {
        Ok(c) => c,
        Err(e) => {
            log::error!("toast renderer: X11 connect failed: {e}");
            return;
        }
    };

    let screen = conn.setup().roots[screen_num].clone();
    let sw = screen.width_in_pixels  as u32;
    let sh = screen.height_in_pixels as u32;
    let depth  = screen.root_depth;
    let visual = screen.root_visual;

    // ── DPI scaling ────────────────────────────────────────────────────────────
    // Read Xft.dpi from the X resource database (RESOURCE_MANAGER property on
    // root). Scale everything so the toast looks correct on HiDPI displays.
    let xft_dpi = read_xft_dpi(&conn, screen.root).unwrap_or(96.0);
    let scale   = xft_dpi / 96.0;
    if (scale - 1.0).abs() > 0.01 {
        log::info!("toast renderer: Xft.dpi={xft_dpi:.0}, scale={scale:.3}");
        // Scale config values that map to physical pixels.
        cfg.width     = (cfg.width    as f64 * scale) as u32;
        cfg.gap       = (cfg.gap      as f64 * scale) as i32;
        cfg.margin_x  = (cfg.margin_x as f64 * scale) as i32;
        cfg.margin_y  = (cfg.margin_y as f64 * scale) as i32;
        cfg.theme.padding       *= scale;
        cfg.theme.border_radius *= scale;
        cfg.theme.border_width  *= scale;
    }

    // Pre-intern atoms for window type hints.
    let atoms = intern_atoms(&conn);

    // Load keyboard mapping for inline reply text input.
    let min_keycode = conn.setup().min_keycode;
    let max_keycode = conn.setup().max_keycode;
    let n_keys      = (max_keycode - min_keycode) as usize + 1;
    let (keysym_table, keysyms_per_key): (Vec<u32>, usize) = {
        match conn.get_keyboard_mapping(min_keycode, n_keys as u8) {
            Ok(cookie) => match cookie.reply() {
                Ok(km) => (km.keysyms, km.keysyms_per_keycode as usize),
                Err(e) => { log::warn!("toast renderer: keyboard mapping reply: {e}"); (vec![], 0) }
            },
            Err(e) => { log::warn!("toast renderer: keyboard mapping request: {e}"); (vec![], 0) }
        }
    };

    let mut windows: Vec<ToastWindow> = Vec::new();
    let mut needs_restack      = false;
    let mut last_progress_tick = Instant::now();

    loop {
        // 1. Drain incoming commands.
        loop {
            match rx.try_recv() {
                Ok(ToastCommand::Show { notif, timeout_ms, group_count }) => {
                    let notif = *notif;
                    let active_count = windows.iter().filter(|w| w.leaving_start.is_none()).count();
                    if active_count >= cfg.max_visible as usize {
                        let oldest_idx = windows
                            .iter()
                            .position(|w| w.leaving_start.is_none() && w.notif.urgency != Urgency::Critical);
                        if let Some(idx) = oldest_idx {
                            windows[idx].leaving_start = Some(Instant::now());
                            needs_restack = true;
                        }
                    }

                    let tw = create_toast(
                        &conn,
                        &screen,
                        depth,
                        visual,
                        &notif,
                        &cfg,
                        &atoms,
                        timeout_ms,
                        scale,
                        group_count,
                    );
                    match tw {
                        Ok(tw) => {
                            windows.push(tw);
                            needs_restack = true;
                        }
                        Err(e) => log::error!("create toast: {e}"),
                    }
                }
                Ok(ToastCommand::Close(id)) => {
                    if let Some(tw) = windows.iter_mut()
                        .find(|w| w.notif.id == id && w.leaving_start.is_none())
                    {
                        tw.leaving_start = Some(Instant::now());
                        needs_restack = true;
                    }
                }
                Ok(ToastCommand::SetFocus(id)) => {
                    for tw in &mut windows {
                        tw.focused = Some(tw.notif.id) == id;
                    }
                    needs_restack = true;
                }
                Ok(ToastCommand::UpdateGroupCount { id, count }) => {
                    if let Some(tw) = windows.iter_mut()
                        .find(|w| w.notif.id == id && w.leaving_start.is_none())
                    {
                        tw.group_count = count;
                        // Force a repaint of this window.
                        let _ = conn.clear_area(true, tw.win, 0, 0, 0, 0);
                        needs_restack = true;
                    }
                }
                Err(mpsc::error::TryRecvError::Empty)        => break,
                Err(mpsc::error::TryRecvError::Disconnected) => return,
            }
        }

        // 2. Expire timed-out toasts (skip hovered windows — hover-pause).
        {
            let now = Instant::now();
            for tw in windows.iter_mut() {
                if !tw.hovered
                    && tw.leaving_start.is_none()
                    && tw.expires_at.map(|t| now >= t).unwrap_or(false)
                {
                    tw.leaving_start = Some(now);
                    let _ = event_tx.blocking_send(ToastEvent::Expired(tw.notif.id));
                    needs_restack = true;
                }
            }
        }

        // 3. Reposition + redraw all windows after stack changes.
        if needs_restack {
            restack_windows(&conn, &mut windows, &cfg, sw, sh, depth, &atoms, scale);
            needs_restack = false;
        }

        // 4. Process X11 events.
        loop {
            match conn.poll_for_event() {
                Err(e) => {
                    log::error!("X11 poll_for_event: {e}");
                    break;
                }
                Ok(None) => break,
                Ok(Some(event)) => {
                    match event {
                        Event::Expose(e) => {
                            if let Some(tw) = windows.iter().find(|w| w.win == e.window)
                                && let Err(err) = paint_toast(&conn, tw, &cfg.theme, scale) {
                                    log::warn!("paint toast: {err}");
                                }
                        }

                        Event::ButtonPress(e) => {
                            let wi = windows.iter().position(|w| w.win == e.event && w.leaving_start.is_none());
                            if let Some(wi) = wi {
                                let id = windows[wi].notif.id;

                                if windows[wi].reply_active {
                                    // Click inside the input box → reposition cursor.
                                    let h         = windows[wi].rect.height as f64;
                                    let btn_row_h = BTN_ROW_H * scale;
                                    let input_y   = h - btn_row_h + (btn_row_h - BTN_H) * scale / 2.0;
                                    let input_h   = BTN_H * scale;
                                    let in_input  = (e.event_y as f64) >= input_y
                                        && (e.event_y as f64) < input_y + input_h;
                                    if in_input {
                                        let text_offset_x = cfg.theme.padding + 8.0 * scale;
                                        let input_w       = windows[wi].rect.width as f64 - cfg.theme.padding * 2.0;
                                        let max_iw        = input_w - 16.0 * scale;
                                        let x_rel         = e.event_x as f64 - text_offset_x;
                                        let bp = byte_index_at_input_x(
                                            &windows[wi].reply_text, x_rel, max_iw,
                                            BTN_SIZE * scale);
                                        windows[wi].cursor_pos    = bp;
                                        windows[wi].sel_anchor    = Some(bp);
                                        windows[wi].dragging_reply = true;
                                        paint_toast(&conn, &windows[wi], &cfg.theme, scale).ok();
                                        conn.flush().ok();
                                    }
                                } else {
                                    // Determine which button (if any) was hit.
                                    let hit_idx = windows[wi].button_rects.iter().find(|btn| {
                                        e.event_x >= btn.x
                                            && e.event_x < btn.x + btn.w as i16
                                            && e.event_y >= btn.y
                                            && e.event_y < btn.y + btn.h as i16
                                    }).map(|btn| btn.idx);

                                    let action_key = hit_idx
                                        .filter(|&i| i != REPLY_BTN_IDX)
                                        .and_then(|i| windows[wi].notif.actions.get(i))
                                        .map(|a| a.key.clone());

                                    let default_key = windows[wi].notif.actions
                                        .iter()
                                        .find(|a| a.key == "default")
                                        .map(|a| a.key.clone());

                                    match hit_idx {
                                        Some(i) if i == REPLY_BTN_IDX => {
                                            // Enter inline reply mode: show input box, grab focus.
                                            windows[wi].reply_active = true;
                                            windows[wi].reply_text.clear();
                                            let win = windows[wi].win;
                                            paint_toast(&conn, &windows[wi], &cfg.theme, scale).ok();
                                            conn.set_input_focus(
                                                InputFocus::PARENT,
                                                win,
                                                0u32,
                                            ).ok();
                                            conn.flush().ok();
                                        }
                                        Some(_) => {
                                            if let Some(key) = action_key {
                                                let _ = event_tx.blocking_send(
                                                    ToastEvent::ActionInvoked { id, key },
                                                );
                                            }
                                        }
                                        None => {
                                            // Body click → invoke "default" or dismiss.
                                            if let Some(key) = default_key {
                                                let _ = event_tx.blocking_send(
                                                    ToastEvent::ActionInvoked { id, key },
                                                );
                                            } else {
                                                let _ = event_tx.blocking_send(
                                                    ToastEvent::Dismissed(id),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        Event::KeyPress(e) => {
                            let wi = windows.iter().position(|w| w.win == e.event && w.reply_active);
                            if let Some(wi) = wi {
                                let id = windows[wi].notif.id;
                                let action = keycode_to_action(
                                    e.detail,
                                    u16::from(e.state),
                                    min_keycode,
                                    &keysym_table,
                                    keysyms_per_key,
                                );
                                let mut repaint = true;
                                match action {
                                    KeyAction::Char(c) => {
                                        // If there is an active selection, delete it first.
                                        let tw = &mut windows[wi];
                                        if let Some((s, e_)) = sel_range_of(tw) {
                                            tw.reply_text.drain(s..e_);
                                            tw.cursor_pos = s;
                                        }
                                        tw.sel_anchor = None;
                                        let pos = tw.cursor_pos;
                                        tw.reply_text.insert(pos, c);
                                        tw.cursor_pos = pos + c.len_utf8();
                                    }
                                    KeyAction::Backspace => {
                                        let tw = &mut windows[wi];
                                        if let Some((s, e_)) = sel_range_of(tw) {
                                            tw.reply_text.drain(s..e_);
                                            tw.cursor_pos = s;
                                            tw.sel_anchor = None;
                                        } else {
                                            tw.sel_anchor = None;
                                            let pos = tw.cursor_pos;
                                            if pos > 0 {
                                                let prev = tw.reply_text[..pos]
                                                    .char_indices().next_back()
                                                    .map(|(i, _)| i).unwrap_or(0);
                                                tw.reply_text.drain(prev..pos);
                                                tw.cursor_pos = prev;
                                            }
                                        }
                                    }
                                    KeyAction::Delete => {
                                        let tw = &mut windows[wi];
                                        if let Some((s, e_)) = sel_range_of(tw) {
                                            tw.reply_text.drain(s..e_);
                                            tw.cursor_pos = s;
                                            tw.sel_anchor = None;
                                        } else {
                                            tw.sel_anchor = None;
                                            let pos = tw.cursor_pos;
                                            let len = tw.reply_text.len();
                                            if pos < len {
                                                let next = tw.reply_text[pos..]
                                                    .char_indices().nth(1)
                                                    .map(|(i, _)| pos + i)
                                                    .unwrap_or(len);
                                                tw.reply_text.drain(pos..next);
                                            }
                                        }
                                    }
                                    KeyAction::CursorLeft => {
                                        let tw = &mut windows[wi];
                                        tw.sel_anchor = None;
                                        let pos = tw.cursor_pos;
                                        if pos > 0 {
                                            let prev = tw.reply_text[..pos]
                                                .char_indices().next_back()
                                                .map(|(i, _)| i).unwrap_or(0);
                                            tw.cursor_pos = prev;
                                        }
                                    }
                                    KeyAction::CursorRight => {
                                        let tw = &mut windows[wi];
                                        tw.sel_anchor = None;
                                        let pos = tw.cursor_pos;
                                        let len = tw.reply_text.len();
                                        if pos < len {
                                            let next = tw.reply_text[pos..]
                                                .char_indices().nth(1)
                                                .map(|(i, _)| pos + i)
                                                .unwrap_or(len);
                                            tw.cursor_pos = next;
                                        }
                                    }
                                    KeyAction::CursorHome => {
                                        windows[wi].sel_anchor  = None;
                                        windows[wi].cursor_pos  = 0;
                                    }
                                    KeyAction::CursorEnd => {
                                        windows[wi].sel_anchor  = None;
                                        windows[wi].cursor_pos  = windows[wi].reply_text.len();
                                    }
                                    KeyAction::Enter => {
                                        let text = std::mem::take(&mut windows[wi].reply_text);
                                        windows[wi].cursor_pos  = 0;
                                        windows[wi].sel_anchor  = None;
                                        windows[wi].reply_active = false;
                                        paint_toast(&conn, &windows[wi], &cfg.theme, scale).ok();
                                        conn.flush().ok();
                                        if !text.trim().is_empty() {
                                            let _ = event_tx.blocking_send(
                                                ToastEvent::ReplySubmitted { id, text },
                                            );
                                        }
                                        repaint = false;
                                    }
                                    KeyAction::Escape => {
                                        windows[wi].reply_active = false;
                                        windows[wi].reply_text.clear();
                                        windows[wi].cursor_pos  = 0;
                                        windows[wi].sel_anchor  = None;
                                    }
                                    KeyAction::ClearLine => {
                                        windows[wi].reply_text.clear();
                                        windows[wi].cursor_pos  = 0;
                                        windows[wi].sel_anchor  = None;
                                    }
                                    KeyAction::DeleteWord => {
                                        windows[wi].sel_anchor = None;
                                        let pos = windows[wi].cursor_pos;
                                        let t = &windows[wi].reply_text[..pos];
                                        let trimmed_len = t.trim_end_matches(' ').len();
                                        let new_pos = match t[..trimmed_len].rfind(' ') {
                                            Some(p) => p + 1,
                                            None    => 0,
                                        };
                                        windows[wi].reply_text.drain(new_pos..pos);
                                        windows[wi].cursor_pos = new_pos;
                                    }
                                    KeyAction::SelectAll => {
                                        windows[wi].sel_anchor  = Some(0);
                                        windows[wi].cursor_pos  = windows[wi].reply_text.len();
                                    }
                                    KeyAction::Copy => {
                                        if let Some((s, e_)) = sel_range_of(&windows[wi]) {
                                            let text = windows[wi].reply_text[s..e_].to_owned();
                                            write_clipboard(&text);
                                        }
                                    }
                                    KeyAction::Paste => {
                                        let clip = read_clipboard();
                                        if !clip.is_empty() {
                                            let tw = &mut windows[wi];
                                            if let Some((s, e_)) = sel_range_of(tw) {
                                                tw.reply_text.drain(s..e_);
                                                tw.cursor_pos = s;
                                                tw.sel_anchor = None;
                                            }
                                            let pos = tw.cursor_pos;
                                            let clean: String = clip.chars()
                                                .filter(|c| !c.is_control())
                                                .collect();
                                            tw.reply_text.insert_str(pos, &clean);
                                            tw.cursor_pos = pos + clean.len();
                                        }
                                    }
                                    KeyAction::Other => { repaint = false; }
                                }
                                if repaint {
                                    paint_toast(&conn, &windows[wi], &cfg.theme, scale).ok();
                                    conn.flush().ok();
                                }
                            }
                        }

                        Event::MotionNotify(e) => {
                            let wi = windows.iter().position(|w| {
                                w.win == e.event && w.reply_active && w.dragging_reply
                            });
                            if let Some(wi) = wi {
                                let text_offset_x = cfg.theme.padding + 8.0 * scale;
                                let input_w       = windows[wi].rect.width as f64 - cfg.theme.padding * 2.0;
                                let max_iw        = input_w - 16.0 * scale;
                                let x_rel         = e.event_x as f64 - text_offset_x;
                                let bp = byte_index_at_input_x(
                                    &windows[wi].reply_text, x_rel, max_iw,
                                    BTN_SIZE * scale);
                                windows[wi].cursor_pos = bp;
                                paint_toast(&conn, &windows[wi], &cfg.theme, scale).ok();
                                conn.flush().ok();
                            }
                        }

                        Event::ButtonRelease(e) => {
                            if let Some(tw) = windows.iter_mut().find(|w| w.win == e.event) {
                                tw.dragging_reply = false;
                                // Collapse degenerate (zero-length) selection.
                                if tw.sel_anchor == Some(tw.cursor_pos) {
                                    tw.sel_anchor = None;
                                }
                            }
                        }

                        Event::EnterNotify(e) => {
                            if let Some(tw) = windows.iter_mut().find(|w| w.win == e.event) {
                                tw.hovered     = true;
                                tw.hover_start = Some(Instant::now());
                                paint_toast(&conn, tw, &cfg.theme, scale).ok();
                                conn.flush().ok();
                            }
                        }

                        Event::LeaveNotify(e) => {
                            if let Some(tw) = windows.iter_mut().find(|w| w.win == e.event) {
                                // Extend the deadline by however long we were hovered.
                                if let (Some(start), Some(expires_at)) =
                                    (tw.hover_start.take(), tw.expires_at)
                                {
                                    tw.expires_at = Some(expires_at + start.elapsed());
                                }
                                tw.hovered = false;
                                paint_toast(&conn, tw, &cfg.theme, scale).ok();
                                conn.flush().ok();
                            }
                        }

                        _ => {}
                    }
                }
            }
        }

        let _ = conn.flush();

        // ── Animations + progress bar repaints ─────────────────────────────────
        const ANIM_SECS:  f64 = 0.18;
        const LEAVE_SECS: f64 = 0.15;
        const MOVE_SECS:  f64 = 0.20;  // smooth restack
        let mut needs_flush = false;

        for tw in &mut windows {
            if let Some(start) = tw.leaving_start {
                let t     = (start.elapsed().as_secs_f64() / LEAVE_SECS).min(1.0);
                let frame = cfg.animation.leave_frame(
                    t, tw.rect.x, tw.rect.y, tw.rect.width, tw.rect.height, sw, sh,
                );
                conn.configure_window(
                    tw.win,
                    &xproto::ConfigureWindowAux::new().x(frame.x).y(frame.y),
                ).ok();
                if cfg.animation.uses_opacity() {
                    set_opacity(&conn, tw.win, frame.opacity, &atoms);
                }
                needs_flush = true;
            } else if let Some(start) = tw.anim_start {
                let elapsed = start.elapsed().as_secs_f64();
                if elapsed < ANIM_SECS {
                    let t     = elapsed / ANIM_SECS;
                    let frame = cfg.animation.enter_frame(
                        t, tw.rect.x, tw.rect.y, tw.rect.width, tw.rect.height, sw, sh,
                    );
                    conn.configure_window(
                        tw.win,
                        &xproto::ConfigureWindowAux::new().x(frame.x).y(frame.y),
                    ).ok();
                    if cfg.animation.uses_opacity() {
                        set_opacity(&conn, tw.win, frame.opacity, &atoms);
                    }
                    needs_flush = true;
                } else if cfg.animation.uses_opacity() {
                    set_opacity(&conn, tw.win, 1.0, &atoms);
                }
            }
            // Smooth Y-restack: cubic ease-in-out between old and target Y.
            if let (Some(target_y), Some(start)) = (tw.target_y, tw.move_start) {
                let elapsed = start.elapsed().as_secs_f64();
                if elapsed < MOVE_SECS {
                    let t    = elapsed / MOVE_SECS;
                    let ease = if t < 0.5 {
                        4.0 * t * t * t
                    } else {
                        1.0 - (-2.0 * t + 2.0_f64).powi(3) / 2.0
                    };
                    let orig_y = tw.rect.y;
                    let cur_y  = orig_y + ((target_y - orig_y) as f64 * ease) as i32;
                    conn.configure_window(
                        tw.win,
                        &xproto::ConfigureWindowAux::new().y(cur_y),
                    ).ok();
                    needs_flush = true;
                } else {
                    conn.configure_window(
                        tw.win,
                        &xproto::ConfigureWindowAux::new().y(target_y),
                    ).ok();
                    tw.rect.y     = target_y;
                    tw.target_y   = None;
                    tw.move_start = None;
                    needs_flush   = true;
                }
            }
        }

        // Destroy windows whose leave animation has completed.
        {
            let now = Instant::now();
            let leave_done: Vec<usize> = windows
                .iter()
                .enumerate()
                .filter(|(_, tw)| {
                    tw.leaving_start
                        .map(|s| now.duration_since(s).as_secs_f64() >= LEAVE_SECS)
                        .unwrap_or(false)
                })
                .map(|(i, _)| i)
                .collect();
            for &idx in leave_done.iter().rev() {
                let tw = windows.remove(idx);
                destroy_window(&conn, tw.win, tw.gc);
            }
            if !leave_done.is_empty() {
                needs_restack = true;
            }
        }

        // Progress bar repaint ~4 fps (every 250 ms).
        if last_progress_tick.elapsed() >= Duration::from_millis(250) {
            last_progress_tick = Instant::now();
            for tw in &windows {
                if tw.expires_at.is_some() && !tw.hovered {
                    paint_toast(&conn, tw, &cfg.theme, scale).ok();
                    needs_flush = true;
                }
            }
        }

        if needs_flush {
            conn.flush().ok();
        }

        std::thread::sleep(Duration::from_millis(16));
    }
}

// ── Window creation ────────────────────────────────────────────────────────────

struct Atoms {
    net_wm_window_type:              xproto::Atom,
    net_wm_window_type_notification: xproto::Atom,
    net_wm_state:                    xproto::Atom,
    net_wm_state_above:              xproto::Atom,
    net_wm_window_opacity:           xproto::Atom,
}

fn intern_atoms(conn: &RustConnection) -> Atoms {
    macro_rules! intern {
        ($name:expr) => {{
            conn.intern_atom(false, $name)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| r.atom)
                .unwrap_or(0)
        }};
    }
    Atoms {
        net_wm_window_type:              intern!(b"_NET_WM_WINDOW_TYPE"),
        net_wm_window_type_notification: intern!(b"_NET_WM_WINDOW_TYPE_NOTIFICATION"),
        net_wm_state:                    intern!(b"_NET_WM_STATE"),
        net_wm_state_above:              intern!(b"_NET_WM_STATE_ABOVE"),
        net_wm_window_opacity:           intern!(b"_NET_WM_WINDOW_OPACITY"),
    }
}

// ── Opacity helper ────────────────────────────────────────────────────────────

/// Set `_NET_WM_WINDOW_OPACITY` on a window (0.0 = transparent, 1.0 = opaque).
fn set_opacity(conn: &RustConnection, win: xproto::Window, alpha: f64, atoms: &Atoms) {
    let val = (alpha.clamp(0.0, 1.0) * u32::MAX as f64) as u32;
    conn.change_property32(
        PropMode::REPLACE,
        win,
        atoms.net_wm_window_opacity,
        AtomEnum::CARDINAL,
        &[val],
    ).ok();
}

// ── Layout constants (logical px at 96 DPI — multiplied by scale at runtime) ──

const ACCENT_W:   f64 = 4.0;
const ACCENT_GAP: f64 = 10.0;
const APP_SIZE:   f64 = 10.0;
const TITLE_SIZE: f64 = 13.0;
const BODY_SIZE:  f64 = 11.0;
const BTN_SIZE:   f64 = 10.0;
const BLOCK_GAP:  f64 = 7.0;
const BTN_ROW_H:  f64 = 44.0;
const BTN_H:      f64 = 26.0;
const BTN_GAP:    f64 = 8.0;
const BTN_PAD_X:  f64 = 14.0;
const CHAR_W_EST: f64 = 6.0;

fn measure_height(notif: &Notification, cfg: &ToastConfig, scale: f64) -> u32 {
    let pad = cfg.theme.padding;  // already scaled in run()
    let s   = scale;
    let mut h = pad;
    h += APP_SIZE   * s + BLOCK_GAP * s;
    h += TITLE_SIZE * s + BLOCK_GAP * s;
    if !notif.body.is_empty() {
        h += BODY_SIZE * s + BLOCK_GAP * s;
    }
    h += pad;
    if !notif.actions.is_empty() || notif.can_reply {
        h += BTN_ROW_H * s;
    }
    (h as u32).max((80.0 * s) as u32)
}

/// Sentinel index value marking the inline-reply button inside `button_rects`.
const REPLY_BTN_IDX: usize = usize::MAX;

fn compute_button_rects(notif: &Notification, cfg: &ToastConfig, scale: f64) -> Vec<ButtonRect> {
    if notif.actions.is_empty() && !notif.can_reply {
        return vec![];
    }
    let h       = measure_height(notif, cfg, scale) as i16;
    let btn_h   = (BTN_H * scale) as i16;
    let row_h   = (BTN_ROW_H * scale) as i16;
    let btn_y   = h - row_h + (row_h - btn_h) / 2;
    let pad     = cfg.theme.padding as i16;
    let btn_gap = (BTN_GAP * scale) as i16;
    let pad_x   = (BTN_PAD_X * scale) as i16;
    let min_w   = (70.0 * scale) as i16;

    let mut rects = Vec::new();
    let mut x = pad;

    for (idx, action) in notif.actions.iter().enumerate() {
        let cols = action.label.chars().count() as f64;
        let w    = ((cols * CHAR_W_EST * scale) as i16 + pad_x * 2).max(min_w) as u16;
        rects.push(ButtonRect { x, y: btn_y, w, h: (BTN_H * scale) as u16, idx });
        x += w as i16 + btn_gap;
    }

    if notif.can_reply {
        // "↩ Reply" label — estimate width from char count.
        let reply_label = "↩ Reply";
        let cols = reply_label.chars().count() as f64;
        let w    = ((cols * CHAR_W_EST * scale) as i16 + pad_x * 2).max(min_w) as u16;
        rects.push(ButtonRect { x, y: btn_y, w, h: (BTN_H * scale) as u16, idx: REPLY_BTN_IDX });
    }

    rects
}

#[allow(clippy::too_many_arguments)]
fn create_toast(
    conn:       &RustConnection,
    screen:     &xproto::Screen,
    depth:      u8,
    visual:     xproto::Visualid,
    notif:      &Notification,
    cfg:        &ToastConfig,
    atoms:      &Atoms,
    timeout_ms: Option<u64>,
    scale:      f64,
    group_count: u32,
) -> anyhow::Result<ToastWindow> {
    let height = measure_height(notif, cfg, scale);
    let rect   = ToastRect { x: 0, y: 0, width: cfg.width, height };

    let win = conn.generate_id()?;
    let gc  = conn.generate_id()?;

    conn.create_window(
        depth,
        win,
        screen.root,
        rect.x as i16,
        rect.y as i16,
        rect.width as u16,
        rect.height as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        visual,
        &CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(
                EventMask::EXPOSURE
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::BUTTON1_MOTION
                    | EventMask::ENTER_WINDOW
                    | EventMask::LEAVE_WINDOW
                    | EventMask::KEY_PRESS,
            ),
    )?;

    conn.create_gc(gc, win, &CreateGCAux::new())?;

    conn.change_property32(
        PropMode::REPLACE,
        win,
        atoms.net_wm_window_type,
        AtomEnum::ATOM,
        &[atoms.net_wm_window_type_notification],
    )?;

    conn.change_property32(
        PropMode::REPLACE,
        win,
        atoms.net_wm_state,
        AtomEnum::ATOM,
        &[atoms.net_wm_state_above],
    )?;

    let expires_at   = timeout_ms.map(|ms| Instant::now() + Duration::from_millis(ms));
    let button_rects = compute_button_rects(notif, cfg, scale);

    Ok(ToastWindow {
        win,
        gc,
        notif: notif.clone(),
        rect,
        expires_at,
        created_at:  Instant::now(),
        depth,
        hovered:     false,
        hover_start: None,
        button_rects,
        anim_start:    None,   // set in restack_windows when position is first assigned
        leaving_start: None,
        target_y:      None,
        move_start:    None,
        focused:       false,
        reply_active:  false,
        reply_text:    String::new(),
        cursor_pos:    0,
        sel_anchor:    None,
        dragging_reply: false,
        group_count,
    })
}

// ── Stacking + painting ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn restack_windows(
    conn:    &RustConnection,
    windows: &mut [ToastWindow],
    cfg:     &ToastConfig,
    sw:      u32,
    sh:      u32,
    _depth:  u8,
    atoms:   &Atoms,
    scale:   f64,
) {
    let mut slot = 0usize;
    for tw in windows.iter_mut() {
        if tw.leaving_start.is_some() {
            continue;  // leaving toasts keep their last position
        }
        let rect = position_for(cfg, slot, tw.rect.height, sw, sh);
        // First positioning → start animation from the style's initial position.
        if tw.anim_start.is_none() {
            tw.anim_start = Some(Instant::now());
            let (init_x, init_y) = cfg.animation.initial_pos(
                rect.x, rect.y, rect.width, rect.height, sw, sh,
            );
            // For fade variants: make window invisible before mapping.
            if cfg.animation.uses_opacity() {
                set_opacity(conn, tw.win, 0.0, atoms);
            }
            conn.configure_window(
                tw.win,
                &xproto::ConfigureWindowAux::new()
                    .x(init_x)
                    .y(init_y)
                    .width(rect.width)
                    .height(rect.height),
            ).ok();
            conn.map_window(tw.win).ok();
        } else {
            // Already visible: if Y changed, start a smooth move.
            if rect.y != tw.rect.y && tw.move_start.is_none() {
                tw.target_y  = Some(rect.y);
                tw.move_start = Some(Instant::now());
            }
            // X and dimensions can update immediately (handles width changes).
            conn.configure_window(
                tw.win,
                &xproto::ConfigureWindowAux::new()
                    .x(rect.x)
                    .width(rect.width)
                    .height(rect.height),
            ).ok();
            conn.map_window(tw.win).ok();
        }
        tw.rect = rect;
        paint_toast(conn, tw, &cfg.theme, scale).ok();
        slot += 1;
    }
    conn.flush().ok();
}

fn destroy_window(conn: &RustConnection, win: xproto::Window, gc: xproto::Gcontext) {
    conn.unmap_window(win).ok();
    conn.free_gc(gc).ok();
    conn.destroy_window(win).ok();
    conn.flush().ok();
}

// ── Cairo rendering ────────────────────────────────────────────────────────────

fn paint_toast(
    conn:  &RustConnection,
    tw:    &ToastWindow,
    theme: &ThemeConfig,
    scale: f64,
) -> anyhow::Result<()> {
    let w = tw.rect.width  as i32;
    let h = tw.rect.height as i32;

    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h)
        .map_err(|e| anyhow::anyhow!("cairo surface: {e}"))?;

    {
        let ctx = cairo::Context::new(&surface)
            .map_err(|e| anyhow::anyhow!("cairo context: {e}"))?;

        let colors = resolve(theme, tw.notif.urgency);
        let wf = w as f64;
        let hf = h as f64;
        let pad = theme.padding;

        // ── Background ─────────────────────────────────────────────────────────
        ctx.set_source_rgba(colors.bg.r, colors.bg.g, colors.bg.b, 1.0);
        ctx.set_operator(cairo::Operator::Source);
        rounded_rect(&ctx, 0.0, 0.0, wf, hf, theme.border_radius);
        ctx.fill().ok();
        ctx.set_operator(cairo::Operator::Over);

        // Hover overlay — subtle brightening when mouse is inside.
        if tw.hovered {
            ctx.set_source_rgba(1.0, 1.0, 1.0, 0.06);
            rounded_rect(&ctx, 0.0, 0.0, wf, hf, theme.border_radius);
            ctx.fill().ok();
        }

        // ── Border ─────────────────────────────────────────────────────────────
        let border_w = if tw.focused {
            theme.border_width + (2.0 * scale).max(1.0)
        } else {
            theme.border_width
        };
        let border_color = if tw.focused { colors.accent } else { colors.border };
        ctx.set_source_rgba(border_color.r, border_color.g, border_color.b, 1.0);
        ctx.set_line_width(border_w);
        rounded_rect(&ctx, 0.5, 0.5, wf - 1.0, hf - 1.0, theme.border_radius);
        ctx.stroke().ok();

        // ── Left urgency accent bar ─────────────────────────────────────────────
        let accent_top    = pad;
        let accent_bottom = if tw.notif.actions.is_empty() { hf - pad } else { hf - BTN_ROW_H * scale };
        ctx.set_source_rgba(colors.accent.r, colors.accent.g, colors.accent.b, 1.0);
        let r = (ACCENT_W * scale / 2.0).min((accent_bottom - accent_top) / 2.0);
        rounded_rect(&ctx, pad, accent_top, ACCENT_W * scale, accent_bottom - accent_top, r);
        ctx.fill().ok();

        // ── Icon + text content ───────────────────────────────────────────────
        let icon_size = theme.icon_size as f64 * scale;
        let icon_x    = pad + ACCENT_W * scale + ACCENT_GAP * scale;
        let icon_y    = pad + 1.0 * scale;
        let icon_gap  = 10.0 * scale;
        draw_notification_icon_or_badge(&ctx, &tw.notif, icon_x, icon_y, icon_size, colors.fg, colors.accent);

        let text_x    = icon_x + icon_size + icon_gap;
        let max_tw    = wf - text_x - pad;
        let mut top_y = pad;

        let app_size   = APP_SIZE   * scale;
        let title_size = TITLE_SIZE * scale;
        let body_size  = BODY_SIZE  * scale;
        let btn_size   = BTN_SIZE   * scale;
        let block_gap  = BLOCK_GAP  * scale;
        let btn_row_h  = BTN_ROW_H  * scale;
        let btn_pad_x  = BTN_PAD_X  * scale;

        // App name
        let app_label = format_app_name(&tw.notif.app_name);
        draw_text_clipped(&ctx, &app_label, text_x, top_y + app_size, app_size,
                          cairo::FontWeight::Normal, colors.fg_dim, max_tw);
        top_y += app_size + block_gap;

        // Summary — bold
        draw_text_clipped(&ctx, &tw.notif.summary, text_x, top_y + title_size, title_size,
                          cairo::FontWeight::Bold, colors.fg, max_tw);
        top_y += title_size + block_gap;

        // Body
        if !tw.notif.body.is_empty() {
            draw_text_clipped(&ctx, &tw.notif.body, text_x, top_y + body_size, body_size,
                              cairo::FontWeight::Normal, colors.fg_dim, max_tw);
        }

        // ── Action buttons + reply button / inline input ───────────────────────
        if !tw.notif.actions.is_empty() || tw.notif.can_reply {
            let sep_y = hf - btn_row_h;

            ctx.set_source_rgba(colors.border.r, colors.border.g, colors.border.b, 0.6);
            ctx.set_line_width(1.0);
            ctx.move_to(pad, sep_y);
            ctx.line_to(wf - pad, sep_y);
            ctx.stroke().ok();

            if tw.reply_active {
                // ── Inline text input ─────────────────────────────────────────
                let input_x = pad;
                let input_w = wf - pad * 2.0;
                let input_h = BTN_H * scale;
                let input_y = hf - btn_row_h + (btn_row_h - BTN_H) * scale / 2.0;

                // Background
                ctx.set_source_rgba(colors.fg_dim.r, colors.fg_dim.g, colors.fg_dim.b, 0.08);
                rounded_rect(&ctx, input_x, input_y, input_w, input_h, 6.0 * scale);
                ctx.fill().ok();

                // Active border in accent colour
                ctx.set_source_rgba(colors.accent.r, colors.accent.g, colors.accent.b, 0.85);
                ctx.set_line_width(1.5);
                rounded_rect(&ctx, input_x + 0.5, input_y + 0.5, input_w - 1.0, input_h - 1.0, 6.0 * scale);
                ctx.stroke().ok();

                let text_padding = 8.0 * scale;
                let max_iw       = input_w - text_padding * 2.0;
                let text_baseline_y = input_y + (input_h + btn_size) / 2.0 - 1.0;

                draw_input_content(
                    &ctx,
                    &tw.reply_text,
                    &tw.notif.reply_placeholder,
                    tw.cursor_pos,
                    tw.sel_anchor,
                    input_x + text_padding,
                    text_baseline_y,
                    btn_size,
                    max_iw,
                    colors.fg,
                    colors.fg_dim,
                    colors.accent,
                    scale,
                );
            } else {
                // ── Action buttons (+ reply button) ───────────────────────────
                for btn in &tw.button_rects {
                    let bx = btn.x as f64;
                    let by = btn.y as f64;
                    let bw = btn.w as f64;
                    let bh = btn.h as f64;

                    let is_reply = btn.idx == REPLY_BTN_IDX;

                    if is_reply {
                        ctx.set_source_rgba(colors.accent.r, colors.accent.g, colors.accent.b, 0.18);
                    } else {
                        ctx.set_source_rgba(colors.fg_dim.r, colors.fg_dim.g, colors.fg_dim.b, 0.12);
                    }
                    rounded_rect(&ctx, bx, by, bw, bh, 5.0 * scale);
                    ctx.fill().ok();

                    if is_reply {
                        ctx.set_source_rgba(colors.accent.r, colors.accent.g, colors.accent.b, 0.70);
                    } else {
                        ctx.set_source_rgba(colors.border.r, colors.border.g, colors.border.b, 0.8);
                    }
                    ctx.set_line_width(1.0);
                    rounded_rect(&ctx, bx + 0.5, by + 0.5, bw - 1.0, bh - 1.0, 5.0 * scale);
                    ctx.stroke().ok();

                    let label_y  = by + (bh + btn_size) / 2.0 - 1.0;
                    let text_col = if is_reply { colors.accent } else { colors.fg };
                    let label    = if is_reply {
                        "↩ Reply".to_owned()
                    } else {
                        tw.notif.actions.get(btn.idx)
                            .map(|a| a.label.clone())
                            .unwrap_or_default()
                    };
                    if !label.is_empty() {
                        draw_text_clipped(&ctx, &label, bx + btn_pad_x, label_y,
                                          btn_size, cairo::FontWeight::Normal,
                                          text_col, bw - btn_pad_x * 2.0);
                    }
                }
            }
        }
        // ── Progress bar ───────────────────────────────────────────────────────
        // Shown only when the notification has a finite timeout.
        // Drawn as a 3 px strip at the very bottom of the toast.
        if let Some(expires_at) = tw.expires_at {
            let total_secs = expires_at
                .checked_duration_since(tw.created_at)
                .unwrap_or(Duration::from_secs(1))
                .as_secs_f64();
            let remaining  = expires_at
                .checked_duration_since(Instant::now())
                .unwrap_or(Duration::ZERO)
                .as_secs_f64();
            let progress = (remaining / total_secs).clamp(0.0, 1.0);

            let bar_h = (3.0_f64 * scale).max(2.0);
            // Keep the timeout bar above the stroke so thick/focused borders
            // never cover it visually.
            let border_w = if tw.focused {
                theme.border_width + (2.0 * scale).max(1.0)
            } else {
                theme.border_width
            };
            let bar_y = (hf - bar_h - border_w - 1.0).max(0.0);
            let bar_w = wf - pad * 2.0;

            // Track
            ctx.set_source_rgba(colors.fg_dim.r, colors.fg_dim.g, colors.fg_dim.b, 0.18);
            rounded_rect(&ctx, pad, bar_y, bar_w, bar_h, bar_h / 2.0);
            ctx.fill().ok();

            // Fill
            if progress > 0.0 {
                let fill = bar_w * progress;
                ctx.set_source_rgba(colors.accent.r, colors.accent.g, colors.accent.b, 0.85);
                rounded_rect(&ctx, pad, bar_y, fill, bar_h, bar_h / 2.0);
                ctx.fill().ok();
            }
        }
        // ── Group-stack badge ──────────────────────────────────────────────────
        // Shown in the top-right corner when multiple notifications share a group.
        if tw.group_count > 1 {
            let badge_label = format!("+{}", tw.group_count - 1);
            let badge_r    = 9.0 * scale;
            let badge_x    = wf - badge_r * 2.0 - pad / 2.0;
            let badge_y    = pad / 2.0;
            // Circle background
            ctx.set_source_rgba(colors.accent.r, colors.accent.g, colors.accent.b, 0.90);
            ctx.arc(badge_x + badge_r, badge_y + badge_r, badge_r, 0.0, std::f64::consts::TAU);
            ctx.fill().ok();
            // Label
            let lsz = (7.5 * scale).max(8.0);
            ctx.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
            ctx.set_font_size(lsz);
            let (ext_w, ext_h, ext_xb, ext_yb) = ctx.text_extents(&badge_label)
                .map(|e| (e.width(), e.height(), e.x_bearing(), e.y_bearing()))
                .unwrap_or((0.0, 0.0, 0.0, 0.0));
            let tx  = badge_x + badge_r - ext_w / 2.0 - ext_xb;
            let ty  = badge_y + badge_r + ext_h / 2.0 - ext_yb - ext_h;
            ctx.set_source_rgba(colors.bg.r, colors.bg.g, colors.bg.b, 1.0);
            ctx.move_to(tx, ty);
            ctx.show_text(&badge_label).ok();
        }
        // ctx dropped → surface borrow released
    }

    let pixel_data = surface
        .data()
        .map_err(|e| anyhow::anyhow!("cairo data: {e}"))?;

    conn.put_image(
        ImageFormat::Z_PIXMAP,
        tw.win,
        tw.gc,
        w as u16,
        h as u16,
        0,
        0,
        0,
        tw.depth,
        &pixel_data,
    )?;

    Ok(())
}

// ── Drawing helpers ────────────────────────────────────────────────────────────

fn rounded_rect(ctx: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    ctx.new_sub_path();
    ctx.arc(x + w - r, y + r,     r, -std::f64::consts::PI / 2.0, 0.0);
    ctx.arc(x + w - r, y + h - r, r, 0.0,                         std::f64::consts::PI / 2.0);
    ctx.arc(x + r,     y + h - r, r, std::f64::consts::PI / 2.0,  std::f64::consts::PI);
    ctx.arc(x + r,     y + r,     r, std::f64::consts::PI,        3.0 * std::f64::consts::PI / 2.0);
    ctx.close_path();
}

/// Draw text at the given baseline position (x, baseline_y) using Pango.
/// Clips with "…" if the text would exceed max_width.
/// Supports emoji, complex scripts and proper font metrics.
#[allow(clippy::too_many_arguments)]
fn draw_text_clipped(
    ctx:        &cairo::Context,
    text:       &str,
    x:          f64,
    baseline_y: f64,
    size:       f64,
    weight:     cairo::FontWeight,
    color:      Rgb,
    max_width:  f64,
) {
    let layout = pangocairo::functions::create_layout(ctx);

    // Build font description.
    let mut fd = pango::FontDescription::new();
    fd.set_family("Sans");
    fd.set_size((size * pango::SCALE as f64) as i32);
    if weight == cairo::FontWeight::Bold {
        fd.set_weight(pango::Weight::Bold);
    }
    layout.set_font_description(Some(&fd));

    // Ellipsize if text exceeds max_width.
    layout.set_width((max_width * pango::SCALE as f64) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_single_paragraph_mode(true);
    layout.set_text(text);

    // Pango baseline is at the top of the layout; offset by the ascent so
    // our `baseline_y` coordinate matches the cairo toy-text baseline.
    let (_, logical) = layout.extents();
    let ascent = layout.baseline();
    let top_y = baseline_y - (ascent as f64 / pango::SCALE as f64);

    ctx.set_source_rgba(color.r, color.g, color.b, 1.0);
    ctx.move_to(x, top_y);
    pangocairo::functions::show_layout(ctx, &layout);
    let _ = logical; // suppress unused warning
}

/// Legacy truncation helper — kept for the icon badge call path.
#[allow(dead_code)]
fn truncate_to_width(ctx: &cairo::Context, text: &str, max_width: f64) -> String {
    let ellipsis = "…";
    let ew = ctx.text_extents(ellipsis).map(|e| e.width()).unwrap_or(10.0);
    let budget = (max_width - ew).max(0.0);
    let mut result = String::new();
    let mut used   = 0.0_f64;
    for ch in text.chars() {
        let s: String = std::iter::once(ch).collect();
        let cw = ctx.text_extents(&s).map(|e| e.width()).unwrap_or(6.0);
        if used + cw > budget { break; }
        result.push(ch);
        used += cw;
    }
    result.push_str(ellipsis);
    result
}

fn draw_notification_icon_or_badge(
    ctx: &cairo::Context,
    notif: &Notification,
    x: f64,
    y: f64,
    size: f64,
    fg: Rgb,
    accent: Rgb,
) {
    let size_px = size.max(1.0).round() as i32;

    if let Some(surface) = load_notification_icon_surface(notif, size_px) {
        let _ = ctx.save();
        let _ = ctx.set_source_surface(&surface, x, y);
        let _ = ctx.paint();
        let _ = ctx.restore();
        return;
    }

    // Fallback badge when the system icon cannot be resolved.
    ctx.set_source_rgba(accent.r, accent.g, accent.b, 0.22);
    rounded_rect(ctx, x, y, size, size, (6.0 * (size / 32.0)).min(size / 2.0));
    ctx.fill().ok();

    let ch = format_app_name(&notif.app_name)
        .chars()
        .next()
        .unwrap_or('•')
        .to_string();
    draw_text_clipped(
        ctx,
        &ch,
        x + size * 0.30,
        y + size * 0.72,
        size * 0.55,
        cairo::FontWeight::Bold,
        fg,
        size * 0.5,
    );
}

fn load_notification_icon_surface(notif: &Notification, size_px: i32) -> Option<cairo::ImageSurface> {
    let icon = resolve_icon(&notif.icon, &notif.app_name, size_px as u32)?;
    let mut surface = cairo::ImageSurface::create(
        cairo::Format::ARgb32,
        icon.width as i32,
        icon.height as i32,
    ).ok()?;
    let stride = surface.stride() as usize;
    {
        let mut data = surface.data().ok()?;
        for y in 0..icon.height as usize {
            let row_src = y * icon.width as usize * 4;
            let row_dst = y * stride;
            let len = icon.width as usize * 4;
            data[row_dst..row_dst + len].copy_from_slice(&icon.pixels[row_src..row_src + len]);
        }
    }
    Some(surface)
}

/// Format app name: title-case, trim long names.
fn format_app_name(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Read `Xft.dpi` from the X resource database stored on the root window.
/// Returns the DPI value, or `None` if unavailable.
fn read_xft_dpi(conn: &RustConnection, root: x11rb::protocol::xproto::Window) -> Option<f64> {
    use x11rb::protocol::xproto::AtomEnum;
    let reply = conn
        .get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 65536)
        .ok()?
        .reply()
        .ok()?;
    let data = std::str::from_utf8(&reply.value).ok()?;
    for line in data.lines() {
        // Format: "Xft.dpi:\t120" or "Xft.dpi: 120"
        if let Some(rest) = line.strip_prefix("Xft.dpi:")
            && let Ok(dpi) = rest.trim().parse::<f64>() {
                return Some(dpi);
            }
    }
    None
}

// ── Keyboard helpers for inline reply input ───────────────────────────────────

enum KeyAction {
    Char(char),
    Backspace,
    Delete,     // Del key — remove char after cursor
    Enter,
    Escape,
    CursorLeft,
    CursorRight,
    CursorHome,
    CursorEnd,
    ClearLine,   // Ctrl+U
    DeleteWord,  // Ctrl+W / Ctrl+Backspace
    SelectAll,   // Ctrl+A
    Copy,        // Ctrl+C
    Paste,       // Ctrl+V
    Other,
}

/// Translate an X11 keycode + modifier state to a `KeyAction`.
/// Uses the keyboard mapping loaded at renderer startup.
fn keycode_to_action(
    keycode:      u8,
    state:        u16,
    min_keycode:  u8,
    table:        &[u32],
    keysyms_per_key: usize,
) -> KeyAction {
    if keysyms_per_key == 0 || keycode < min_keycode {
        return KeyAction::Other;
    }
    let ctrl  = state & 0x0004 != 0;
    let shift = state & 0x0001 != 0 && !ctrl;
    let base  = (keycode - min_keycode) as usize * keysyms_per_key;
    // Base (unshifted) keysym — used for Ctrl shortcuts and special keys.
    let base_ks  = table.get(base).copied().unwrap_or(0);
    // Shifted keysym — used for printable chars when Shift is held.
    let shift_ks = table.get(base + 1).copied().unwrap_or(base_ks);
    let keysym   = if shift { shift_ks } else { base_ks };

    // Ctrl+key shortcuts — match on the *base* keysym (letter regardless of shift).
    if ctrl {
        return match base_ks {
            0x0076 | 0x0056 => KeyAction::Paste,       // Ctrl+V
            0x0075 | 0x0055 => KeyAction::ClearLine,   // Ctrl+U
            0x0077 | 0x0057 => KeyAction::DeleteWord,  // Ctrl+W
            0x0061 | 0x0041 => KeyAction::SelectAll,   // Ctrl+A
            0x0063 | 0x0043 => KeyAction::Copy,        // Ctrl+C
            0xff08           => KeyAction::DeleteWord,  // Ctrl+Backspace
            0xff0d | 0xff8d  => KeyAction::Enter,
            0xff1b           => KeyAction::Escape,
            _                => KeyAction::Other,
        };
    }

    match keysym {
        0xff08 => KeyAction::Backspace,
        0xffff => KeyAction::Delete,
        0xff0d | 0xff8d => KeyAction::Enter,
        0xff1b => KeyAction::Escape,
        0xff51 => KeyAction::CursorLeft,
        0xff53 => KeyAction::CursorRight,
        0xff50 => KeyAction::CursorHome,
        0xff57 => KeyAction::CursorEnd,
        _ => keysym_to_char(keysym).map(KeyAction::Char).unwrap_or(KeyAction::Other),
    }
}

/// Convert an X11 keysym to a Unicode `char`.
///
/// Handles:
/// - ASCII printable (0x20–0x7e)
/// - Latin-1 supplement (0xa0–0xff, same as Unicode)
/// - Unicode keysyms 0x01xxxxxx (emitted by modern xkb)
/// - Legacy Latin-2 keysyms (0x01xx) for Polish and other CE characters
fn keysym_to_char(ks: u32) -> Option<char> {
    // Unicode keysyms: 0x01000000 | codepoint (X11R6.4+, modern xkb default)
    if (ks & 0xff00_0000) == 0x0100_0000 {
        return char::from_u32(ks & 0x00ff_ffff);
    }
    // Direct ASCII
    if (0x0020..=0x007e).contains(&ks) {
        return char::from_u32(ks);
    }
    // Latin-1 supplement = Unicode
    if (0x00a0..=0x00ff).contains(&ks) {
        return char::from_u32(ks);
    }
    // Legacy Latin-2 keysyms → Unicode codepoints (covers Polish + other CE scripts)
    let cp: u32 = match ks {
        0x01a1 => 0x0104, 0x01a2 => 0x02d8, 0x01a3 => 0x0141, 0x01a5 => 0x013d,
        0x01a6 => 0x015a, 0x01a9 => 0x0160, 0x01aa => 0x015e, 0x01ab => 0x0164,
        0x01ac => 0x0179, 0x01ae => 0x017d, 0x01af => 0x017b,
        0x01b1 => 0x0105, 0x01b2 => 0x02db, 0x01b3 => 0x0142, 0x01b5 => 0x013e,
        0x01b6 => 0x015b, 0x01b9 => 0x0161, 0x01ba => 0x015f, 0x01bb => 0x0165,
        0x01bc => 0x017a, 0x01be => 0x017e, 0x01bf => 0x017c,
        0x01c0 => 0x0154, 0x01c3 => 0x0102, 0x01c5 => 0x0139, 0x01c6 => 0x0106,
        0x01c8 => 0x010c, 0x01ca => 0x0118, 0x01cc => 0x011a, 0x01cf => 0x010e,
        0x01d0 => 0x0110, 0x01d1 => 0x0143, 0x01d2 => 0x0147, 0x01d5 => 0x0150,
        0x01d8 => 0x0158, 0x01d9 => 0x016e, 0x01db => 0x0170, 0x01de => 0x0162,
        0x01e0 => 0x0155, 0x01e3 => 0x0103, 0x01e5 => 0x013a, 0x01e6 => 0x0107,
        0x01e8 => 0x010d, 0x01ea => 0x0119, 0x01ec => 0x011b, 0x01ef => 0x010f,
        0x01f0 => 0x0111, 0x01f1 => 0x0144, 0x01f2 => 0x0148, 0x01f5 => 0x0151,
        0x01f8 => 0x0159, 0x01f9 => 0x016f, 0x01fb => 0x0171, 0x01fe => 0x0163,
        _ => 0,
    };
    if cp != 0 { char::from_u32(cp) } else { None }
}

/// Read text from the X11 CLIPBOARD selection.
/// Tries xclip first, falls back to xsel.
fn read_clipboard() -> String {
    if let Ok(out) = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
        && out.status.success()
    {
        let s = String::from_utf8_lossy(&out.stdout).into_owned();
        if !s.is_empty() { return s; }
    }
    if let Ok(out) = std::process::Command::new("xsel")
        .args(["--clipboard", "--output"])
        .output()
        && out.status.success()
    {
        return String::from_utf8_lossy(&out.stdout).into_owned();
    }
    String::new()
}

/// Write `text` to the X11 CLIPBOARD selection.
fn write_clipboard(text: &str) {
    if let Ok(mut child) = std::process::Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(text.as_bytes()).ok();
        }
        child.wait().ok();
    }
}

/// Return the active selection byte range `[start, end)` for a toast window,
/// or `None` if there is no (non-empty) selection.
fn sel_range_of(tw: &ToastWindow) -> Option<(usize, usize)> {
    let anchor = tw.sel_anchor?;
    if anchor == tw.cursor_pos { return None; }
    let (s, e) = (anchor.min(tw.cursor_pos), anchor.max(tw.cursor_pos));
    Some((s, e.min(tw.reply_text.len())))
}

/// Given a pixel offset `x_relative` from the left edge of the input text area,
/// return the UTF-8 byte index in `text` closest to that position.
/// Uses a temporary Cairo surface so font metrics match the rendered output.
fn byte_index_at_input_x(text: &str, x_relative: f64, max_width: f64, font_size: f64) -> usize {
    let Ok(surf) = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1) else {
        return text.len();
    };
    let Ok(ctx) = cairo::Context::new(&surf) else { return text.len(); };
    let layout = pangocairo::functions::create_layout(&ctx);
    let mut fd = pango::FontDescription::new();
    fd.set_family("Sans");
    fd.set_absolute_size(font_size * pango::SCALE as f64);
    layout.set_font_description(Some(&fd));
    layout.set_width((max_width * pango::SCALE as f64) as i32);
    layout.set_single_paragraph_mode(true);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_text(text);

    let x_pango = (x_relative.max(0.0) * pango::SCALE as f64) as i32;
    let (_inside, index, trailing) = layout.xy_to_index(x_pango, 0);
    let idx = (index as usize).min(text.len());
    if trailing > 0 && idx < text.len() {
        text[idx..].char_indices().nth(1)
            .map(|(i, _)| idx + i)
            .unwrap_or(text.len())
    } else {
        idx
    }
}

/// Render the inline reply input field: text (or placeholder), selection
/// highlight, and a proper cursor line — without the `"|"` append hack.
#[allow(clippy::too_many_arguments)]
fn draw_input_content(
    ctx: &cairo::Context,
    text: &str,
    placeholder: &str,
    cursor_pos: usize,
    sel_anchor: Option<usize>,
    x: f64,
    baseline_y: f64,
    font_size: f64,
    max_width: f64,
    fg: Rgb,
    fg_dim: Rgb,
    accent: Rgb,
    scale: f64,
) {
    let layout = pangocairo::functions::create_layout(ctx);
    let mut fd = pango::FontDescription::new();
    fd.set_family("Sans");
    fd.set_absolute_size(font_size * pango::SCALE as f64);
    layout.set_font_description(Some(&fd));
    layout.set_width((max_width * pango::SCALE as f64) as i32);
    layout.set_single_paragraph_mode(true);
    layout.set_ellipsize(pango::EllipsizeMode::End);

    let is_placeholder = text.is_empty();
    let display = if is_placeholder { placeholder } else { text };
    layout.set_text(display);

    // Compute top-left of the text block (Pango ascent from baseline).
    let baseline_pango = layout.baseline();
    let top_y = baseline_y - baseline_pango as f64 / pango::SCALE as f64;

    // ── Selection highlight (behind the text) ─────────────────────────────────
    if !is_placeholder {
        if let (Some(anchor), true) = (sel_anchor, sel_anchor != Some(cursor_pos)) {
            let s = anchor.min(cursor_pos).min(display.len());
            let e = anchor.max(cursor_pos).min(display.len());
            let sr = layout.index_to_pos(s as i32);
            let er = layout.index_to_pos(e as i32);
            let sx = x + sr.x() as f64 / pango::SCALE as f64;
            let ex = x + er.x() as f64 / pango::SCALE as f64;
            let h  = layout.size().1 as f64 / pango::SCALE as f64;
            ctx.set_source_rgba(accent.r, accent.g, accent.b, 0.35);
            ctx.rectangle(sx.min(ex), top_y, (sx - ex).abs().max(1.0), h);
            ctx.fill().ok();
        }
    }

    // ── Text ──────────────────────────────────────────────────────────────────
    let alpha = if is_placeholder { 0.45 } else { 1.0 };
    let col   = if is_placeholder { fg_dim } else { fg };
    ctx.set_source_rgba(col.r, col.g, col.b, alpha);
    ctx.move_to(x, top_y);
    pangocairo::functions::show_layout(ctx, &layout);

    // ── Cursor line ───────────────────────────────────────────────────────────
    let cur_idx = if is_placeholder {
        // Show cursor at end of placeholder text to signal readiness.
        display.len()
    } else {
        cursor_pos.min(display.len())
    };
    let cur = layout.index_to_pos(cur_idx as i32);
    let cx  = x + cur.x() as f64 / pango::SCALE as f64;
    let h   = layout.size().1 as f64 / pango::SCALE as f64;
    let cur_alpha = if is_placeholder { 0.55 } else { 1.0 };
    ctx.set_source_rgba(fg.r, fg.g, fg.b, cur_alpha);
    ctx.set_line_width((1.5 * scale).max(1.0));
    ctx.move_to(cx, top_y + 1.0 * scale);
    ctx.line_to(cx, top_y + h - 1.0 * scale);
    ctx.stroke().ok();
}

