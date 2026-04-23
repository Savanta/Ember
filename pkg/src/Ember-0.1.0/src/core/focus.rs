/// Tracks which toast window (if any) currently has keyboard focus.
/// Phase 1: simple index into the visible toast stack.
/// Phase 2+: integrate with the input controller for full keyboard nav.
#[derive(Debug, Default)]
pub struct FocusManager {
    focused_id: Option<u32>,
}

#[allow(dead_code)]
impl FocusManager {
    pub fn focused(&self) -> Option<u32> {
        self.focused_id
    }

    pub fn set_focus(&mut self, id: Option<u32>) {
        self.focused_id = id;
    }

    pub fn clear(&mut self) {
        self.focused_id = None;
    }

    /// Ensure the focused ID still exists; otherwise focus the first active item.
    pub fn ensure_valid(&mut self, ordered_ids: &[u32]) -> Option<u32> {
        if ordered_ids.is_empty() {
            self.focused_id = None;
            return None;
        }

        if let Some(id) = self.focused_id
            && ordered_ids.contains(&id) {
                return Some(id);
            }

        self.focused_id = Some(ordered_ids[0]);
        self.focused_id
    }

    pub fn focus_next_in(&mut self, ordered_ids: &[u32]) -> Option<u32> {
        if ordered_ids.is_empty() {
            self.focused_id = None;
            return None;
        }

        let idx = self
            .focused_id
            .and_then(|id| ordered_ids.iter().position(|x| *x == id))
            .map(|i| (i + 1) % ordered_ids.len())
            .unwrap_or(0);

        self.focused_id = Some(ordered_ids[idx]);
        self.focused_id
    }

    pub fn focus_prev_in(&mut self, ordered_ids: &[u32]) -> Option<u32> {
        if ordered_ids.is_empty() {
            self.focused_id = None;
            return None;
        }

        let idx = self
            .focused_id
            .and_then(|id| ordered_ids.iter().position(|x| *x == id))
            .map(|i| if i == 0 { ordered_ids.len() - 1 } else { i - 1 })
            .unwrap_or(ordered_ids.len() - 1);

        self.focused_id = Some(ordered_ids[idx]);
        self.focused_id
    }

    /// Move focus when a notification is dismissed.
    pub fn on_dismissed(&mut self, id: u32) {
        if self.focused_id == Some(id) {
            self.focused_id = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_no_focus() {
        let fm = FocusManager::default();
        assert_eq!(fm.focused_id, None);
    }

    #[test]
    fn ensure_valid_empty_list_clears_focus() {
        let mut fm = FocusManager::default();
        fm.set_focus(Some(1));
        assert_eq!(fm.ensure_valid(&[]), None);
        assert_eq!(fm.focused_id, None);
    }

    #[test]
    fn ensure_valid_keeps_existing_valid_focus() {
        let mut fm = FocusManager::default();
        fm.set_focus(Some(2));
        assert_eq!(fm.ensure_valid(&[1, 2, 3]), Some(2));
    }

    #[test]
    fn ensure_valid_reassigns_to_first_when_stale() {
        let mut fm = FocusManager::default();
        fm.set_focus(Some(99));
        assert_eq!(fm.ensure_valid(&[1, 2, 3]), Some(1));
    }

    #[test]
    fn focus_next_wraps_around() {
        let mut fm = FocusManager::default();
        let ids = [1, 2, 3];
        fm.set_focus(Some(3));
        assert_eq!(fm.focus_next_in(&ids), Some(1));
    }

    #[test]
    fn focus_next_advances() {
        let mut fm = FocusManager::default();
        let ids = [1, 2, 3];
        fm.set_focus(Some(1));
        assert_eq!(fm.focus_next_in(&ids), Some(2));
    }

    #[test]
    fn focus_prev_wraps_around() {
        let mut fm = FocusManager::default();
        let ids = [1, 2, 3];
        fm.set_focus(Some(1));
        assert_eq!(fm.focus_prev_in(&ids), Some(3));
    }

    #[test]
    fn focus_prev_retreats() {
        let mut fm = FocusManager::default();
        let ids = [1, 2, 3];
        fm.set_focus(Some(3));
        assert_eq!(fm.focus_prev_in(&ids), Some(2));
    }

    #[test]
    fn on_dismissed_clears_when_focused() {
        let mut fm = FocusManager::default();
        fm.set_focus(Some(5));
        fm.on_dismissed(5);
        assert_eq!(fm.focused_id, None);
    }

    #[test]
    fn on_dismissed_noop_when_not_focused() {
        let mut fm = FocusManager::default();
        fm.set_focus(Some(5));
        fm.on_dismissed(7);
        assert_eq!(fm.focused_id, Some(5));
    }

    #[test]
    fn clear_removes_focus() {
        let mut fm = FocusManager::default();
        fm.set_focus(Some(1));
        fm.clear();
        assert_eq!(fm.focused_id, None);
    }
}
