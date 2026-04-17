/// Derive a group key for a notification.
/// Phase 1: group solely by app_name.
/// Phase 2+: use conversation/thread hints from messaging apps.
pub fn derive_group_key(app_name: &str, hints: &std::collections::HashMap<String, String>) -> Option<String> {
    // Honour an explicit "x-ember-group" hint if present
    if let Some(g) = hints.get("x-ember-group")
        && !g.is_empty() {
            return Some(g.clone());
        }
    // Default: group by application
    if app_name.is_empty() {
        None
    } else {
        Some(app_name.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn groups_by_app_name_lowercase() {
        let hints = HashMap::new();
        assert_eq!(derive_group_key("Discord", &hints), Some("discord".into()));
        assert_eq!(derive_group_key("Slack", &hints), Some("slack".into()));
    }

    #[test]
    fn empty_app_name_returns_none() {
        let hints = HashMap::new();
        assert_eq!(derive_group_key("", &hints), None);
    }

    #[test]
    fn x_ember_group_hint_overrides_app_name() {
        let mut hints = HashMap::new();
        hints.insert("x-ember-group".into(), "team-chat".into());
        assert_eq!(derive_group_key("Slack", &hints), Some("team-chat".into()));
    }

    #[test]
    fn empty_x_ember_group_hint_falls_back_to_app_name() {
        let mut hints = HashMap::new();
        hints.insert("x-ember-group".into(), "".into());
        assert_eq!(derive_group_key("Slack", &hints), Some("slack".into()));
    }
}
