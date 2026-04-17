/// Mouse event handling — managed by the toast renderer for Phase 1.
/// Phase 2+: global mouse hooks, hover state, click-through regions.
#[allow(dead_code)]
pub struct MouseHandler;

#[allow(dead_code)]
impl MouseHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MouseHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_handler_new_and_default_work() {
        let _a = MouseHandler::new();
        let _b = MouseHandler::default();
    }
}
