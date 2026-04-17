use crate::config::ShortcutConfig;

/// Keyboard shortcut definitions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    FocusNext,
    FocusPrev,
    DismissFocused,
    InvokeDefault,
    InvokeAction(u8),
    OpenCenter,
    ToggleDnd,
    ClearAll,
}

#[derive(Debug, Clone)]
pub struct Shortcut {
    pub action: ShortcutAction,
    /// X11 keysym string, e.g. "super+grave".
    pub binding: String,
}

/// Build active shortcut definitions from config.
/// Set any binding to an empty string to disable it.
pub fn shortcuts_from_config(cfg: &ShortcutConfig) -> Vec<Shortcut> {
    let all = vec![
        Shortcut { action: ShortcutAction::FocusNext,      binding: cfg.focus_next.clone() },
        Shortcut { action: ShortcutAction::FocusPrev,      binding: cfg.focus_prev.clone() },
        Shortcut { action: ShortcutAction::DismissFocused, binding: cfg.dismiss_focused.clone() },
        Shortcut { action: ShortcutAction::InvokeDefault,  binding: cfg.invoke_default.clone() },
        Shortcut { action: ShortcutAction::InvokeAction(0), binding: cfg.action_1.clone() },
        Shortcut { action: ShortcutAction::InvokeAction(1), binding: cfg.action_2.clone() },
        Shortcut { action: ShortcutAction::InvokeAction(2), binding: cfg.action_3.clone() },
        Shortcut { action: ShortcutAction::OpenCenter,     binding: cfg.open_center.clone() },
        Shortcut { action: ShortcutAction::ToggleDnd,      binding: cfg.toggle_dnd.clone() },
        Shortcut { action: ShortcutAction::ClearAll,       binding: cfg.clear_all.clone() },
    ];

    all.into_iter()
        .filter(|s| !s.binding.trim().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ShortcutConfig;

    #[test]
    fn shortcuts_from_config_returns_all_defaults() {
        let cfg = ShortcutConfig::default();
        let shortcuts = shortcuts_from_config(&cfg);
        // All 10 default bindings are non-empty, so all 10 should be present
        assert_eq!(shortcuts.len(), 10);
    }

    #[test]
    fn shortcuts_from_config_filters_empty_bindings() {
        let cfg = ShortcutConfig {
            focus_next: "".into(),
            focus_prev: "".into(),
            ..ShortcutConfig::default()
        };
        let shortcuts = shortcuts_from_config(&cfg);
        assert_eq!(shortcuts.len(), 8);
        // FocusNext and FocusPrev should not appear
        assert!(!shortcuts.iter().any(|s| s.action == ShortcutAction::FocusNext));
        assert!(!shortcuts.iter().any(|s| s.action == ShortcutAction::FocusPrev));
    }

    #[test]
    fn shortcuts_from_config_filters_whitespace_only_binding() {
        let cfg = ShortcutConfig {
            clear_all: "   ".into(),
            ..ShortcutConfig::default()
        };
        let shortcuts = shortcuts_from_config(&cfg);
        assert!(!shortcuts.iter().any(|s| s.action == ShortcutAction::ClearAll));
    }

    #[test]
    fn shortcuts_from_config_action_types_are_correct() {
        let cfg = ShortcutConfig::default();
        let shortcuts = shortcuts_from_config(&cfg);

        let has = |action: &ShortcutAction| shortcuts.iter().any(|s| &s.action == action);

        assert!(has(&ShortcutAction::FocusNext));
        assert!(has(&ShortcutAction::FocusPrev));
        assert!(has(&ShortcutAction::DismissFocused));
        assert!(has(&ShortcutAction::InvokeDefault));
        assert!(has(&ShortcutAction::InvokeAction(0)));
        assert!(has(&ShortcutAction::InvokeAction(1)));
        assert!(has(&ShortcutAction::InvokeAction(2)));
        assert!(has(&ShortcutAction::OpenCenter));
        assert!(has(&ShortcutAction::ToggleDnd));
        assert!(has(&ShortcutAction::ClearAll));
    }

    #[test]
    fn shortcuts_from_config_preserves_binding_string() {
        let cfg = ShortcutConfig {
            toggle_dnd: "super+shift+d".into(),
            ..ShortcutConfig::default()
        };
        let shortcuts = shortcuts_from_config(&cfg);
        let toggle = shortcuts.iter().find(|s| s.action == ShortcutAction::ToggleDnd).unwrap();
        assert_eq!(toggle.binding, "super+shift+d");
    }

    #[test]
    fn shortcuts_from_config_empty_config_returns_empty() {
        let cfg = ShortcutConfig {
            focus_next:      "".into(),
            focus_prev:      "".into(),
            dismiss_focused: "".into(),
            invoke_default:  "".into(),
            action_1:        "".into(),
            action_2:        "".into(),
            action_3:        "".into(),
            open_center:     "".into(),
            toggle_dnd:      "".into(),
            clear_all:       "".into(),
        };
        let shortcuts = shortcuts_from_config(&cfg);
        assert!(shortcuts.is_empty());
    }
}
