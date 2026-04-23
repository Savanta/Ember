use crate::config::ThemeConfig;
use crate::store::models::Urgency;

/// Parsed RGB color ready for cairo.
#[derive(Debug, Clone, Copy)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Rgb {
    pub fn from_hex(hex: &str) -> Self {
        let hex = hex.trim_start_matches('#');
        let r = u8::from_str_radix(hex.get(0..2).unwrap_or("88"), 16).unwrap_or(0x88);
        let g = u8::from_str_radix(hex.get(2..4).unwrap_or("88"), 16).unwrap_or(0x88);
        let b = u8::from_str_radix(hex.get(4..6).unwrap_or("88"), 16).unwrap_or(0x88);
        Self {
            r: r as f64 / 255.0,
            g: g as f64 / 255.0,
            b: b as f64 / 255.0,
        }
    }
}

/// Resolved colors for a given notification urgency.
pub struct ResolvedTheme {
    pub bg:     Rgb,
    pub fg:     Rgb,
    pub fg_dim: Rgb,
    pub border: Rgb,
    pub accent: Rgb,
}

pub fn resolve(theme: &ThemeConfig, urgency: Urgency) -> ResolvedTheme {
    let (bg_hex, border_hex, accent_hex) = match urgency {
        Urgency::Low      => (&theme.bg_low,      &theme.border_normal,   &theme.accent_low),
        Urgency::Normal   => (&theme.bg_normal,    &theme.border_normal,   &theme.accent_normal),
        Urgency::Critical => (&theme.bg_critical,  &theme.border_critical, &theme.accent_critical),
    };
    ResolvedTheme {
        bg:     Rgb::from_hex(bg_hex),
        fg:     Rgb::from_hex(&theme.fg),
        fg_dim: Rgb::from_hex(&theme.fg_dim),
        border: Rgb::from_hex(border_hex),
        accent: Rgb::from_hex(accent_hex),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeConfig;

    // ── Rgb::from_hex ─────────────────────────────────────────────────────────

    #[test]
    fn from_hex_pure_red() {
        let c = Rgb::from_hex("#ff0000");
        assert!((c.r - 1.0).abs() < 1e-6);
        assert!(c.g.abs() < 1e-6);
        assert!(c.b.abs() < 1e-6);
    }

    #[test]
    fn from_hex_without_hash_prefix() {
        let c = Rgb::from_hex("00ff00");
        assert!(c.r.abs() < 1e-6);
        assert!((c.g - 1.0).abs() < 1e-6);
        assert!(c.b.abs() < 1e-6);
    }

    #[test]
    fn from_hex_pure_white() {
        let c = Rgb::from_hex("#ffffff");
        assert!((c.r - 1.0).abs() < 1e-6);
        assert!((c.g - 1.0).abs() < 1e-6);
        assert!((c.b - 1.0).abs() < 1e-6);
    }

    #[test]
    fn from_hex_pure_black() {
        let c = Rgb::from_hex("#000000");
        assert!(c.r.abs() < 1e-6);
        assert!(c.g.abs() < 1e-6);
        assert!(c.b.abs() < 1e-6);
    }

    #[test]
    fn from_hex_midpoint_88() {
        // #888888 → 0x88 / 255 ≈ 0.533
        let c = Rgb::from_hex("#888888");
        let expected = 0x88_u8 as f64 / 255.0;
        assert!((c.r - expected).abs() < 1e-6);
        assert!((c.g - expected).abs() < 1e-6);
        assert!((c.b - expected).abs() < 1e-6);
    }

    #[test]
    fn from_hex_invalid_falls_back_to_0x88() {
        // Short/invalid hex → each component defaults to 0x88
        let c = Rgb::from_hex("xyz");
        let expected = 0x88_u8 as f64 / 255.0;
        assert!((c.r - expected).abs() < 1e-6);
    }

    // ── resolve ───────────────────────────────────────────────────────────────

    fn test_theme() -> ThemeConfig {
        ThemeConfig {
            bg_normal:       "#282828".into(),
            bg_low:          "#1d2021".into(),
            bg_critical:     "#cc241d".into(),
            fg:              "#ebdbb2".into(),
            fg_dim:          "#928374".into(),
            border_normal:   "#504945".into(),
            border_critical: "#fb4934".into(),
            accent_normal:   "#d79921".into(),
            accent_low:      "#a89984".into(),
            accent_critical: "#fb4934".into(),
            ..ThemeConfig::default()
        }
    }

    #[test]
    fn resolve_normal_uses_normal_colors() {
        let theme = test_theme();
        let resolved = resolve(&theme, Urgency::Normal);
        // bg should come from bg_normal (#282828 → r=0x28/255)
        let expected_bg = 0x28_u8 as f64 / 255.0;
        assert!((resolved.bg.r - expected_bg).abs() < 1e-6);
    }

    #[test]
    fn resolve_critical_uses_critical_bg() {
        let theme = test_theme();
        let resolved = resolve(&theme, Urgency::Critical);
        // bg_critical = #cc241d → r = 0xcc/255
        let expected_r = 0xcc_u8 as f64 / 255.0;
        assert!((resolved.bg.r - expected_r).abs() < 1e-6);
    }

    #[test]
    fn resolve_low_uses_low_bg() {
        let theme = test_theme();
        let resolved = resolve(&theme, Urgency::Low);
        // bg_low = #1d2021 → r = 0x1d/255
        let expected_r = 0x1d_u8 as f64 / 255.0;
        assert!((resolved.bg.r - expected_r).abs() < 1e-6);
    }

    #[test]
    fn resolve_fg_same_across_all_urgencies() {
        let theme = test_theme();
        let r_normal   = resolve(&theme, Urgency::Normal);
        let r_critical = resolve(&theme, Urgency::Critical);
        let r_low      = resolve(&theme, Urgency::Low);
        // fg is shared, all should be equal
        assert!((r_normal.fg.r   - r_critical.fg.r).abs() < 1e-6);
        assert!((r_normal.fg.r   - r_low.fg.r).abs()      < 1e-6);
    }
}
