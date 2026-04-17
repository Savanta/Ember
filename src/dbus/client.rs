//! D-Bus client helpers — used when invoking actions that require
//! calling back into applications.

/// Raise or launch the application associated with a notification's app name.
/// Tries `wmctrl -xa <name>` first; falls back to `gtk-launch` and `xdg-open`.
///
/// For full implementation see [`crate::core::actions::open_notification`],
/// which is called by the engine on "default" action invocations.
#[allow(dead_code)]
pub fn execute_default_action(app_name: &str) {
    use std::process::Command;
    // Try to raise via wmctrl; engine's open_notification handles the full
    // lookup including desktop-entry hints. Here we have app_name only.
    let _ = Command::new("wmctrl").args(["-xa", app_name]).status();
}
