//! Animation style definitions and per-frame geometry computation.

use serde::Deserialize;

/// Which animation plays when a toast enters or leaves the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnimStyle {
    /// Slide in from the right edge, slide out to the right (default).
    #[default]
    SlideRight,
    /// Slide in from the left edge, slide out to the left.
    SlideLeft,
    /// Slide in downward from above the final position, slide out upward.
    SlideDown,
    /// Slide in upward from the bottom of the screen, slide out downward.
    SlideUp,
    /// Fade in / fade out in place (requires a compositor).
    Fade,
    /// Fade in combined with a 40 px slide from the right (requires a compositor).
    FadeSlide,
}

/// Computed window state for one animation frame.
pub struct AnimFrame {
    /// Target window X position.
    pub x: i32,
    /// Target window Y position.
    pub y: i32,
    /// Target opacity: 0.0 (transparent) … 1.0 (opaque).
    pub opacity: f64,
}

impl AnimStyle {
    /// Horizontal offset used by [`AnimStyle::FadeSlide`].
    const FADE_OFFSET: i32 = 40;

    /// Initial (x, y) placed on window creation before the first frame.
    pub fn initial_pos(
        self,
        final_x: i32, final_y: i32,
        width: u32, height: u32,
        sw: u32, sh: u32,
    ) -> (i32, i32) {
        match self {
            AnimStyle::SlideRight => (sw as i32, final_y),
            AnimStyle::SlideLeft  => (-(width as i32), final_y),
            AnimStyle::SlideDown  => (final_x, final_y - height as i32),
            AnimStyle::SlideUp    => (final_x, sh as i32),
            AnimStyle::Fade       => (final_x, final_y),
            AnimStyle::FadeSlide  => (final_x + Self::FADE_OFFSET, final_y),
        }
    }

    /// Compute window state at enter-animation progress `t` (0 → 1). Cubic ease-out.
    pub fn enter_frame(
        self,
        t: f64,
        final_x: i32, final_y: i32,
        width: u32, height: u32,
        sw: u32, sh: u32,
    ) -> AnimFrame {
        let ease = 1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3);
        match self {
            AnimStyle::SlideRight => AnimFrame {
                x: sw as i32 + ((final_x - sw as i32) as f64 * ease) as i32,
                y: final_y,
                opacity: 1.0,
            },
            AnimStyle::SlideLeft => AnimFrame {
                x: -(width as i32) + ((final_x + width as i32) as f64 * ease) as i32,
                y: final_y,
                opacity: 1.0,
            },
            AnimStyle::SlideDown => AnimFrame {
                x: final_x,
                y: final_y - height as i32 + (height as f64 * ease) as i32,
                opacity: 1.0,
            },
            AnimStyle::SlideUp => AnimFrame {
                x: final_x,
                y: sh as i32 + ((final_y - sh as i32) as f64 * ease) as i32,
                opacity: 1.0,
            },
            AnimStyle::Fade => AnimFrame {
                x: final_x,
                y: final_y,
                opacity: ease,
            },
            AnimStyle::FadeSlide => AnimFrame {
                x: final_x + Self::FADE_OFFSET - (Self::FADE_OFFSET as f64 * ease) as i32,
                y: final_y,
                opacity: ease,
            },
        }
    }

    /// Compute window state at leave-animation progress `t` (0 → 1). Cubic ease-in.
    pub fn leave_frame(
        self,
        t: f64,
        final_x: i32, final_y: i32,
        width: u32, height: u32,
        sw: u32, sh: u32,
    ) -> AnimFrame {
        let ease = t.clamp(0.0, 1.0).powi(3);
        match self {
            AnimStyle::SlideRight => AnimFrame {
                x: final_x + ((sw as i32 - final_x) as f64 * ease) as i32,
                y: final_y,
                opacity: 1.0,
            },
            AnimStyle::SlideLeft => AnimFrame {
                x: final_x - ((final_x + width as i32) as f64 * ease) as i32,
                y: final_y,
                opacity: 1.0,
            },
            AnimStyle::SlideDown => AnimFrame {
                x: final_x,
                y: final_y - (height as f64 * ease) as i32,
                opacity: 1.0,
            },
            AnimStyle::SlideUp => AnimFrame {
                x: final_x,
                y: final_y + ((sh as i32 - final_y) as f64 * ease) as i32,
                opacity: 1.0,
            },
            AnimStyle::Fade => AnimFrame {
                x: final_x,
                y: final_y,
                opacity: 1.0 - ease,
            },
            AnimStyle::FadeSlide => AnimFrame {
                x: final_x + (Self::FADE_OFFSET as f64 * ease) as i32,
                y: final_y,
                opacity: 1.0 - ease,
            },
        }
    }

    /// Whether this style modifies opacity (needs `_NET_WM_WINDOW_OPACITY`).
    pub fn uses_opacity(self) -> bool {
        matches!(self, AnimStyle::Fade | AnimStyle::FadeSlide)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SW: u32 = 1920;
    const SH: u32 = 1080;
    const W:  u32 = 440;
    const H:  u32 = 80;
    const FX: i32 = 100;
    const FY: i32 = 200;

    // ── uses_opacity ─────────────────────────────────────────────────────────

    #[test]
    fn uses_opacity_only_for_fade_styles() {
        assert!(!AnimStyle::SlideRight.uses_opacity());
        assert!(!AnimStyle::SlideLeft.uses_opacity());
        assert!(!AnimStyle::SlideDown.uses_opacity());
        assert!(!AnimStyle::SlideUp.uses_opacity());
        assert!(AnimStyle::Fade.uses_opacity());
        assert!(AnimStyle::FadeSlide.uses_opacity());
    }

    // ── enter_frame at t=1.0 should reach final position ─────────────────────

    fn assert_at_final(style: AnimStyle) {
        let frame = style.enter_frame(1.0, FX, FY, W, H, SW, SH);
        assert_eq!(frame.x, FX, "{style:?} enter_frame(1.0).x");
        assert_eq!(frame.y, FY, "{style:?} enter_frame(1.0).y");
    }

    #[test]
    fn enter_frame_t1_reaches_final_slide_right() { assert_at_final(AnimStyle::SlideRight); }
    #[test]
    fn enter_frame_t1_reaches_final_slide_left()  { assert_at_final(AnimStyle::SlideLeft); }
    #[test]
    fn enter_frame_t1_reaches_final_slide_down()  { assert_at_final(AnimStyle::SlideDown); }
    #[test]
    fn enter_frame_t1_reaches_final_slide_up()    { assert_at_final(AnimStyle::SlideUp); }
    #[test]
    fn enter_frame_t1_reaches_final_fade()        { assert_at_final(AnimStyle::Fade); }
    #[test]
    fn enter_frame_t1_reaches_final_fadeslide()   { assert_at_final(AnimStyle::FadeSlide); }

    // ── enter_frame opacity: opaque at t=1 for Fade ──────────────────────────

    #[test]
    fn enter_frame_fade_opacity_1_at_t1() {
        let f = AnimStyle::Fade.enter_frame(1.0, FX, FY, W, H, SW, SH);
        assert!((f.opacity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn enter_frame_fade_opacity_0_at_t0() {
        let f = AnimStyle::Fade.enter_frame(0.0, FX, FY, W, H, SW, SH);
        assert!(f.opacity.abs() < 1e-6);
    }

    // ── enter_frame clamps t ─────────────────────────────────────────────────

    #[test]
    fn enter_frame_clamps_t_above_1() {
        let f1 = AnimStyle::SlideRight.enter_frame(1.0, FX, FY, W, H, SW, SH);
        let f2 = AnimStyle::SlideRight.enter_frame(2.0, FX, FY, W, H, SW, SH);
        assert_eq!(f1.x, f2.x);
    }

    #[test]
    fn enter_frame_clamps_t_below_0() {
        let f1 = AnimStyle::SlideRight.enter_frame(0.0, FX, FY, W, H, SW, SH);
        let f2 = AnimStyle::SlideRight.enter_frame(-1.0, FX, FY, W, H, SW, SH);
        assert_eq!(f1.x, f2.x);
    }

    // ── leave_frame at t=0.0 should be at final position (no movement yet) ───

    fn assert_leave_at_start(style: AnimStyle) {
        let frame = style.leave_frame(0.0, FX, FY, W, H, SW, SH);
        assert_eq!(frame.x, FX, "{style:?} leave_frame(0.0).x");
        assert_eq!(frame.y, FY, "{style:?} leave_frame(0.0).y");
    }

    #[test]
    fn leave_frame_t0_at_final_slide_right() { assert_leave_at_start(AnimStyle::SlideRight); }
    #[test]
    fn leave_frame_t0_at_final_slide_left()  { assert_leave_at_start(AnimStyle::SlideLeft); }
    #[test]
    fn leave_frame_t0_at_final_fade()        { assert_leave_at_start(AnimStyle::Fade); }

    // ── slide_right: start position is off right edge ────────────────────────

    #[test]
    fn initial_pos_slide_right_starts_off_right_edge() {
        let (ix, _iy) = AnimStyle::SlideRight.initial_pos(FX, FY, W, H, SW, SH);
        assert_eq!(ix, SW as i32);
    }

    #[test]
    fn initial_pos_slide_left_starts_off_left_edge() {
        let (ix, _iy) = AnimStyle::SlideLeft.initial_pos(FX, FY, W, H, SW, SH);
        assert_eq!(ix, -(W as i32));
    }

    #[test]
    fn initial_pos_fade_starts_at_final() {
        let (ix, iy) = AnimStyle::Fade.initial_pos(FX, FY, W, H, SW, SH);
        assert_eq!(ix, FX);
        assert_eq!(iy, FY);
    }
}

