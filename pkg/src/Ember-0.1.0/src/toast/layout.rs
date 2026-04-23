use crate::config::ToastConfig;

/// Screen geometry for a single toast window.
#[derive(Debug, Clone, Copy)]
pub struct ToastRect {
    pub x:      i32,
    pub y:      i32,
    pub width:  u32,
    pub height: u32,
}

/// Which corner of the screen to anchor the toast stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

impl Anchor {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "top-left"     => Self::TopLeft,
            "bottom-right" => Self::BottomRight,
            "bottom-left"  => Self::BottomLeft,
            _              => Self::TopRight,
        }
    }

    pub fn grows_downward(self) -> bool {
        matches!(self, Self::TopRight | Self::TopLeft)
    }

    pub fn anchored_left(self) -> bool {
        matches!(self, Self::TopLeft | Self::BottomLeft)
    }
}

/// Calculate the screen position for toast at `index` in the stack.
pub fn position_for(
    cfg:          &ToastConfig,
    index:        usize,
    toast_height: u32,
    screen_width: u32,
    screen_height: u32,
) -> ToastRect {
    let anchor = Anchor::from_str(&cfg.position);
    let step   = (toast_height as i32) + cfg.gap;

    let x = if anchor.anchored_left() {
        cfg.margin_x
    } else {
        screen_width as i32 - cfg.width as i32 - cfg.margin_x
    };

    let y = if anchor.grows_downward() {
        cfg.margin_y + step * index as i32
    } else {
        screen_height as i32 - cfg.margin_y - toast_height as i32 - step * index as i32
    };

    ToastRect { x, y, width: cfg.width, height: toast_height }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ToastConfig;

    fn cfg_top_right() -> ToastConfig {
        ToastConfig {
            position: "top-right".into(),
            width: 440,
            gap: 10,
            margin_x: 20,
            margin_y: 50,
            ..ToastConfig::default()
        }
    }

    // ── Anchor::from_str ──────────────────────────────────────────────────────

    #[test]
    fn anchor_from_str_all_variants() {
        assert_eq!(Anchor::from_str("top-right"),    Anchor::TopRight);
        assert_eq!(Anchor::from_str("top-left"),     Anchor::TopLeft);
        assert_eq!(Anchor::from_str("bottom-right"), Anchor::BottomRight);
        assert_eq!(Anchor::from_str("bottom-left"),  Anchor::BottomLeft);
    }

    #[test]
    fn anchor_from_str_defaults_to_top_right() {
        assert_eq!(Anchor::from_str(""),         Anchor::TopRight);
        assert_eq!(Anchor::from_str("unknown"),  Anchor::TopRight);
    }

    #[test]
    fn anchor_from_str_is_case_insensitive() {
        assert_eq!(Anchor::from_str("TOP-LEFT"),     Anchor::TopLeft);
        assert_eq!(Anchor::from_str("Bottom-Right"), Anchor::BottomRight);
    }

    // ── Anchor flags ─────────────────────────────────────────────────────────

    #[test]
    fn grows_downward_for_top_anchors() {
        assert!(Anchor::TopRight.grows_downward());
        assert!(Anchor::TopLeft.grows_downward());
        assert!(!Anchor::BottomRight.grows_downward());
        assert!(!Anchor::BottomLeft.grows_downward());
    }

    #[test]
    fn anchored_left_for_left_anchors() {
        assert!(Anchor::TopLeft.anchored_left());
        assert!(Anchor::BottomLeft.anchored_left());
        assert!(!Anchor::TopRight.anchored_left());
        assert!(!Anchor::BottomRight.anchored_left());
    }

    // ── position_for ─────────────────────────────────────────────────────────

    #[test]
    fn position_for_top_right_index_zero() {
        let cfg = cfg_top_right();
        let rect = position_for(&cfg, 0, 80, 1920, 1080);
        // x = screen_width - toast_width - margin_x = 1920 - 440 - 20 = 1460
        assert_eq!(rect.x, 1920 - 440 - 20);
        // y = margin_y + (height + gap) * 0 = 50
        assert_eq!(rect.y, 50);
        assert_eq!(rect.width, 440);
        assert_eq!(rect.height, 80);
    }

    #[test]
    fn position_for_top_right_index_one_stacks_downward() {
        let cfg = cfg_top_right();
        let rect0 = position_for(&cfg, 0, 80, 1920, 1080);
        let rect1 = position_for(&cfg, 1, 80, 1920, 1080);
        // second toast should be below the first
        assert_eq!(rect1.y, rect0.y + 80 + 10); // height + gap
        assert_eq!(rect1.x, rect0.x); // same column
    }

    #[test]
    fn position_for_top_left_anchors_to_left_edge() {
        let cfg = ToastConfig {
            position: "top-left".into(),
            width: 440,
            gap: 10,
            margin_x: 20,
            margin_y: 50,
            ..ToastConfig::default()
        };
        let rect = position_for(&cfg, 0, 80, 1920, 1080);
        assert_eq!(rect.x, 20); // margin_x from left
        assert_eq!(rect.y, 50);
    }

    #[test]
    fn position_for_bottom_right_stacks_upward() {
        let cfg = ToastConfig {
            position: "bottom-right".into(),
            width: 440,
            gap: 10,
            margin_x: 20,
            margin_y: 50,
            ..ToastConfig::default()
        };
        let rect0 = position_for(&cfg, 0, 80, 1920, 1080);
        let rect1 = position_for(&cfg, 1, 80, 1920, 1080);
        // bottom anchor: higher index → lower y (further up)
        assert!(rect1.y < rect0.y);
    }
}
