/// libcanberra integration — plays notification sounds via the freedesktop
/// sound-theme system (PipeWire / PulseAudio / ALSA backend).
///
/// Respects D-Bus Notification hints:
///   • `sound-name`      — freedesktop sound-theme event ID (e.g. "message-new-instant")
///   • `sound-file`      — absolute path to a sound file
///   • `suppress-sound`  — "1" or "true" suppresses the sound for this notification
///
/// Falls back to `[sound].default_sound` from the daemon config when none
/// of the per-notification hints are present.

use std::ffi::CString;
use std::sync::Mutex;

// ── libcanberra opaque types ───────────────────────────────────────────────────

#[repr(C)]
struct CaContext {
    _private: [u8; 0],
}

#[repr(C)]
struct CaProplist {
    _private: [u8; 0],
}

// ── FFI declarations ──────────────────────────────────────────────────────────

#[link(name = "canberra")]
unsafe extern "C" {
    fn ca_context_create(c: *mut *mut CaContext) -> i32;
    fn ca_context_open(c: *mut CaContext) -> i32;
    fn ca_context_destroy(c: *mut CaContext) -> i32;
    fn ca_context_change_proplist(c: *mut CaContext, p: *mut CaProplist) -> i32;

    fn ca_proplist_create(p: *mut *mut CaProplist) -> i32;
    fn ca_proplist_sets(
        p: *mut CaProplist,
        key: *const std::ffi::c_char,
        value: *const std::ffi::c_char,
    ) -> i32;
    fn ca_proplist_destroy(p: *mut CaProplist) -> i32;

    /// Non-blocking: libcanberra copies the proplist internally and returns
    /// immediately; the sound is played asynchronously by the audio backend.
    fn ca_context_play_full(
        c: *mut CaContext,
        id: u32,
        p: *mut CaProplist,
        cb: Option<unsafe extern "C" fn(*mut CaContext, u32, i32, *mut std::ffi::c_void)>,
        userdata: *mut std::ffi::c_void,
    ) -> i32;
}

// ── SoundPlayer ───────────────────────────────────────────────────────────────

/// A thin, thread-safe wrapper around a `ca_context`.
///
/// Construct once at daemon start-up and share via `Arc`. All play calls are
/// serialised by the internal mutex, but since `ca_context_play_full` returns
/// immediately the lock is held for only a few microseconds.
pub struct SoundPlayer {
    ctx: Mutex<*mut CaContext>,
}

// The raw pointer is only accessed while holding the Mutex, so Send + Sync are
// safe to assert manually.
unsafe impl Send for SoundPlayer {}
unsafe impl Sync for SoundPlayer {}

impl SoundPlayer {
    /// Try to initialise a libcanberra context.  Returns `None` if the audio
    /// subsystem is unavailable (e.g. headless CI), so callers can degrade
    /// gracefully without crashing the daemon.
    pub fn new() -> Option<Self> {
        unsafe {
            let mut ctx: *mut CaContext = std::ptr::null_mut();
            if ca_context_create(&mut ctx) != 0 || ctx.is_null() {
                log::warn!("sound: ca_context_create failed — audio disabled");
                return None;
            }

            // Set application identity so the audio server can identify us.
            let mut pl: *mut CaProplist = std::ptr::null_mut();
            if ca_proplist_create(&mut pl) == 0 && !pl.is_null() {
                set_prop(pl, "application.name",  "ember");
                set_prop(pl, "application.id",    "org.ember.notify");
                ca_context_change_proplist(ctx, pl);
                ca_proplist_destroy(pl);
            }

            if ca_context_open(ctx) != 0 {
                log::warn!("sound: ca_context_open failed — audio disabled");
                ca_context_destroy(ctx);
                return None;
            }

            log::info!("sound: libcanberra context opened");
            Some(Self { ctx: Mutex::new(ctx) })
        }
    }

    /// Play a sound for one notification.
    ///
    /// Priority order:
    /// 1. `sound_file` hint (absolute path)
    /// 2. `sound_name` hint (freedesktop theme event ID)
    /// 3. `default_sound` from daemon config
    pub fn play(
        &self,
        sound_name:    Option<&str>,
        sound_file:    Option<&str>,
        default_sound: &str,
    ) {
        let (prop_key, prop_val): (&str, &str) = if let Some(f) = sound_file {
            ("media.filename", f)
        } else {
            let name = sound_name.unwrap_or(default_sound);
            ("event.id", name)
        };

        let Ok(key_c) = CString::new(prop_key) else { return };
        let Ok(val_c) = CString::new(prop_val) else { return };

        let Ok(ctx_guard) = self.ctx.lock() else { return };
        let ctx = *ctx_guard;
        if ctx.is_null() { return; }

        unsafe {
            let mut pl: *mut CaProplist = std::ptr::null_mut();
            if ca_proplist_create(&mut pl) != 0 || pl.is_null() { return; }
            ca_proplist_sets(pl, key_c.as_ptr(), val_c.as_ptr());
            let rc = ca_context_play_full(ctx, 0, pl, None, std::ptr::null_mut());
            ca_proplist_destroy(pl);
            if rc != 0 {
                log::debug!("sound: ca_context_play_full returned {rc}");
            }
        }
    }
}

impl Drop for SoundPlayer {
    fn drop(&mut self) {
        if let Ok(ctx) = self.ctx.lock() {
            if !ctx.is_null() {
                unsafe { ca_context_destroy(*ctx); }
            }
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

unsafe fn set_prop(pl: *mut CaProplist, key: &str, value: &str) {
    if let (Ok(k), Ok(v)) = (CString::new(key), CString::new(value)) {
        unsafe { ca_proplist_sets(pl, k.as_ptr(), v.as_ptr()); }
    }
}
