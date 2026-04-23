//! App icon resolution and rasterization for toast windows.
//!
//! Resolution order:
//! 1. Absolute path to a PNG/SVG/JPEG file (from D-Bus `app_icon` param).
//! 2. XDG icon theme — look for `<name>.png` in hicolor at preferred sizes.
//! 3. Parse `.desktop` file for the app name → extract `Icon=` field → repeat 2.
//! 4. Return `None` if nothing found.

use std::path::{Path, PathBuf};

// ── Public API ────────────────────────────────────────────────────────────────

/// Resolved, rasterized icon as a flat BGRA (cairo ARgb32) pixel buffer.
pub struct IconData {
    pub pixels: Vec<u8>,
    pub width:  u32,
    pub height: u32,
}

/// Resolve and rasterize an icon for a notification.
///
/// `icon_hint` is the D-Bus `app_icon` parameter (may be empty, a path, or a
/// theme icon name).  `app_name` is the application name, used as a fallback
/// to search `.desktop` files.
/// `size_px` is the desired icon size in physical pixels (already scaled).
pub fn resolve_icon(icon_hint: &str, app_name: &str, size_px: u32) -> Option<IconData> {
    let size = size_px.clamp(16, 256);

    // 1. Absolute path
    if icon_hint.starts_with('/')
        && let Some(data) = load_path(Path::new(icon_hint), size) {
            return Some(data);
        }

    // 2. Theme icon name from D-Bus hint
    if !icon_hint.is_empty() && !icon_hint.starts_with('/')
        && let Some(path) = find_icon_file(icon_hint, size)
            && let Some(data) = load_path(&path, size) {
                return Some(data);
            }

    // 3. Derive icon name from app_name → .desktop lookup
    let desktop_icon = icon_from_desktop(app_name);
    if let Some(name) = &desktop_icon {
        if let Some(path) = find_icon_file(name, size)
            && let Some(data) = load_path(&path, size) {
                return Some(data);
            }
        // maybe it's already an absolute path from the .desktop file
        if name.starts_with('/')
            && let Some(data) = load_path(Path::new(name.as_str()), size) {
                return Some(data);
            }
    }

    // 4. Try app_name directly as icon name
    if let Some(path) = find_icon_file(app_name, size)
        && let Some(data) = load_path(&path, size) {
            return Some(data);
        }

    None
}

// ── Icon theme search ─────────────────────────────────────────────────────────

/// Search XDG icon theme directories for a PNG at the closest size.
fn find_icon_file(name: &str, preferred_size: u32) -> Option<PathBuf> {
    // Candidate sizes: prefer close-to-requested, then smallest-larger, then largest
    let size_dirs: &[u32] = &[256, 128, 96, 64, 48, 32, 24, 22, 16];

    let data_dirs = xdg_data_dirs();
    let themes = ["hicolor", "Adwaita", "breeze", "gnome", "oxygen"];

    // Try ordered sizes
    for &sz in size_dirs {
        for dir in &data_dirs {
            for theme in &themes {
                // apps subdirectory; also try status and devices
                for category in &["apps", "actions", "status", "devices", "mimetypes"] {
                    let candidates = [
                        dir.join(format!("icons/{theme}/{sz}x{sz}/{category}/{name}.png")),
                        dir.join(format!("icons/{theme}/{sz}x{sz}@2/{category}/{name}.png")),
                    ];
                    for p in &candidates {
                        if p.exists() {
                            // prefer size closest to requested
                            if sz >= preferred_size || (sz as i32 - preferred_size as i32).abs() <= 32 {
                                return Some(p.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    // Second pass — accept anything
    for &sz in size_dirs {
        for dir in &data_dirs {
            for theme in &themes {
                for category in &["apps", "actions", "status", "devices", "mimetypes"] {
                    let candidates = [
                        dir.join(format!("icons/{theme}/{sz}x{sz}/{category}/{name}.png")),
                        dir.join(format!("icons/{theme}/{sz}x{sz}@2/{category}/{name}.png")),
                    ];
                    for p in &candidates {
                        if p.exists() {
                            return Some(p.clone());
                        }
                    }
                }
            }
        }
    }

    // Also try pixmaps
    for dir in &data_dirs {
        let p = dir.join(format!("pixmaps/{name}.png"));
        if p.exists() {
            return Some(p);
        }
    }

    None
}

/// XDG data directories: $XDG_DATA_HOME + $XDG_DATA_DIRS + /usr/share fallback.
fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(home) = dirs::data_dir() {
        dirs.push(home);
    }

    let sys = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for p in sys.split(':') {
        if !p.is_empty() {
            dirs.push(PathBuf::from(p));
        }
    }

    dirs
}

// ── .desktop file lookup ──────────────────────────────────────────────────────

/// Find `Icon=` value from a `.desktop` file matching `app_name`.
fn icon_from_desktop(app_name: &str) -> Option<String> {
    let data_dirs = xdg_data_dirs();
    let needle = app_name.to_lowercase().replace(' ', "-");

    for dir in &data_dirs {
        let apps_dir = dir.join("applications");
        let Ok(rd) = std::fs::read_dir(&apps_dir) else { continue };
        for entry in rd.flatten() {
            let fname = entry.file_name();
            let fname = fname.to_string_lossy().to_lowercase();
            // Match: exact, or starts with app_name, or contains app_name.
            if (fname == format!("{needle}.desktop")
                || fname.starts_with(&format!("{needle}-"))
                || fname.contains(&needle))
                && let Some(icon) = parse_desktop_icon(&entry.path()) {
                    return Some(icon);
                }
        }
    }
    None
}

/// Extract `Icon=` from a `.desktop` file.
fn parse_desktop_icon(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        if let Some(val) = line.strip_prefix("Icon=") {
            let val = val.trim();
            if !val.is_empty() {
                return Some(val.to_owned());
            }
        }
    }
    None
}

// ── Image loading ─────────────────────────────────────────────────────────────

/// Load an image from `path`, scale to `size`×`size`, return BGRA pixels.
fn load_path(path: &Path, size: u32) -> Option<IconData> {
    use image::imageops::FilterType;

    let img = image::open(path).ok()?;
    let img = img.resize(size, size, FilterType::Lanczos3);
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();

    // Convert RGBA → BGRA (cairo ARgb32 in little-endian is stored BGRA).
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for px in rgba.pixels() {
        let [r, g, b, a] = px.0;
        // Pre-multiply alpha (cairo expects it).
        let af = a as f32 / 255.0;
        pixels.push((b as f32 * af) as u8); // B
        pixels.push((g as f32 * af) as u8); // G
        pixels.push((r as f32 * af) as u8); // R
        pixels.push(a);                      // A
    }

    Some(IconData { pixels, width: w, height: h })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toast::icon::load_path;

    // ── resolve_icon: nonexistent path returns None ───────────────────────────

    #[test]
    fn resolve_icon_absolute_nonexistent_path_returns_none() {
        let result = resolve_icon("/nonexistent/path/icon.png", "test-app", 32);
        assert!(result.is_none());
    }

    #[test]
    fn resolve_icon_empty_hint_unknown_app_returns_none() {
        let result = resolve_icon("", "ember-nonexistent-app-xyz-test", 32);
        assert!(result.is_none());
    }

    #[test]
    fn resolve_icon_unknown_theme_name_returns_none() {
        let result = resolve_icon("ember-nonexistent-icon-xyz", "test-app", 32);
        assert!(result.is_none());
    }

    // ── resolve_icon: valid PNG file is loaded ────────────────────────────────

    #[test]
    fn resolve_icon_valid_png_path_returns_icon_data() {
        // Create a minimal 4×4 RGBA PNG in a temp file
        let dir = std::env::temp_dir();
        let path = dir.join("ember-test-icon.png");
        write_minimal_png(&path);

        let result = resolve_icon(path.to_str().unwrap(), "test-app", 32);
        assert!(result.is_some(), "should load a real PNG via absolute path");
        let data = result.unwrap();
        assert_eq!(data.width, 32);
        assert_eq!(data.height, 32);
        // BGRA: 4 bytes per pixel
        assert_eq!(data.pixels.len() as u32, data.width * data.height * 4);

        let _ = std::fs::remove_file(&path);
    }

    // ── size clamping ─────────────────────────────────────────────────────────

    #[test]
    fn resolve_icon_size_clamps_to_16_minimum() {
        let dir = std::env::temp_dir();
        let path = dir.join("ember-test-icon-small.png");
        write_minimal_png(&path);

        // Request size 4 → clamped to 16
        let result = resolve_icon(path.to_str().unwrap(), "test-app", 4);
        if let Some(data) = result {
            assert_eq!(data.width, 16);
            assert_eq!(data.height, 16);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resolve_icon_size_clamps_to_256_maximum() {
        let dir = std::env::temp_dir();
        let path = dir.join("ember-test-icon-large.png");
        write_minimal_png(&path);

        // Request size 1024 → clamped to 256
        let result = resolve_icon(path.to_str().unwrap(), "test-app", 1024);
        if let Some(data) = result {
            assert_eq!(data.width, 256);
            assert_eq!(data.height, 256);
        }
        let _ = std::fs::remove_file(&path);
    }

    // ── parse_desktop_icon ────────────────────────────────────────────────────

    #[test]
    fn parse_desktop_icon_extracts_icon_field() {
        let dir = std::env::temp_dir();
        let path = dir.join("ember-test-app.desktop");
        std::fs::write(&path, "[Desktop Entry]\nName=Test App\nIcon=test-app-icon\nExec=test\n").unwrap();

        let icon = parse_desktop_icon(&path);
        assert_eq!(icon.as_deref(), Some("test-app-icon"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_desktop_icon_trims_whitespace() {
        let dir = std::env::temp_dir();
        let path = dir.join("ember-test-app2.desktop");
        std::fs::write(&path, "[Desktop Entry]\nIcon=  my-icon  \nExec=test\n").unwrap();

        let icon = parse_desktop_icon(&path);
        assert_eq!(icon.as_deref(), Some("my-icon"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_desktop_icon_returns_none_if_no_icon_field() {
        let dir = std::env::temp_dir();
        let path = dir.join("ember-test-app3.desktop");
        std::fs::write(&path, "[Desktop Entry]\nName=Test\nExec=test\n").unwrap();

        let icon = parse_desktop_icon(&path);
        assert!(icon.is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_desktop_icon_returns_none_for_empty_icon_value() {
        let dir = std::env::temp_dir();
        let path = dir.join("ember-test-app4.desktop");
        std::fs::write(&path, "[Desktop Entry]\nIcon=\nExec=test\n").unwrap();

        let icon = parse_desktop_icon(&path);
        assert!(icon.is_none());
        let _ = std::fs::remove_file(&path);
    }

    // ── xdg_data_dirs ────────────────────────────────────────────────────────

    #[test]
    fn xdg_data_dirs_is_non_empty() {
        let dirs = xdg_data_dirs();
        assert!(!dirs.is_empty());
    }

    // ── load_path ────────────────────────────────────────────────────────────

    #[test]
    fn load_path_nonexistent_returns_none() {
        let result = load_path(Path::new("/nonexistent/path.png"), 32);
        assert!(result.is_none());
    }

    #[test]
    fn load_path_valid_png_returns_bgra_pixels() {
        let dir = std::env::temp_dir();
        let path = dir.join("ember-test-load-path.png");
        write_minimal_png(&path);

        let result = load_path(&path, 32);
        assert!(result.is_some());
        let data = result.unwrap();
        assert_eq!(data.width, 32);
        assert_eq!(data.height, 32);
        assert_eq!(data.pixels.len(), (32 * 32 * 4) as usize);
        let _ = std::fs::remove_file(&path);
    }

    // ── Helper: write a minimal valid 1×1 white PNG ───────────────────────────

    fn write_minimal_png(path: &std::path::Path) {
        // Use the `image` crate to write a proper 1×1 white RGBA PNG
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
        img.save(path).expect("write test PNG");
    }
}
