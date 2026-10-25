//! Freedesktop icon-theme lookup, decoded to RGBA bitmaps.
//!
//! Implements a practical subset of the icon-theme spec: we search
//! `<base>/icons/<theme>/**` for `<name>.{png,svg}`, prefer a raster icon whose
//! directory size is closest to the requested size, treat any SVG as always
//! available (it scales), then fall back to the `hicolor` theme and finally the
//! legacy `/usr/share/pixmaps` directory.
//!
//! Performance: each theme directory is walked **once** and the results are
//! kept in a name → best-candidate index ([`ThemeIndex`]), so resolving N
//! icons costs one directory walk instead of N. Large themes (Papirus ships
//! ~60,000 files) made the walk-per-name scheme crawl on the first frame —
//! most visibly on the first launch after boot, when the page cache is cold.

// Icon/thumbnail machinery is compiled in every build so the shared core
// stays one codebase, but it is exercised only by the GUI frontend; core
// (tui-only) builds therefore legitimately contain unreached items here.
#![cfg_attr(not(feature = "gui"), allow(dead_code))]
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A decoded RGBA bitmap ready to be drawn into a tiny-skia pixmap.
/// Pixels are **premultiplied** alpha (tiny-skia's native format).
#[derive(Debug, Clone)]
pub struct Icon {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub struct IconFinder {
    /// base directories that may contain `icons/<theme>/…` and `pixmaps/…`
    bases: Vec<PathBuf>,
    /// theme chain, e.g. ["Papirus", "hicolor"]
    themes: Vec<String>,
    size: u32,
    cache: HashMap<String, Option<Icon>>,
    /// one [`ThemeIndex`] per theme directory, built on the first lookup
    /// that needs it (gui builds only)
    #[cfg(feature = "gui")]
    indexes: HashMap<PathBuf, ThemeIndex>,
}

/// Best candidate per icon name inside one theme directory — the product of
/// a single filesystem walk, so lookups never touch the disk again.
#[derive(Default)]
#[cfg(feature = "gui")]
struct ThemeIndex {
    /// icon name -> (score, path); lower score wins:
    /// 0 = exact-size PNG, 1 = any SVG (scales), 8+distance = other PNG
    best: HashMap<String, (u32, PathBuf)>,
    /// names that exist here as XPM (recorded, never decoded — only used
    /// for the "XPM-only" warning when nothing else matched)
    xpm: HashSet<String>,
}

impl IconFinder {
    pub fn new(theme: &str, size: u32) -> Self {
        Self::with_bases(default_bases(), theme, size)
    }

    /// Injectable bases so tests can point at a temporary icon tree.
    pub fn with_bases(bases: Vec<PathBuf>, theme: &str, size: u32) -> Self {
        let themes = theme_chain(&bases, theme);
        Self {
            bases,
            themes,
            size,
            cache: HashMap::new(),
            #[cfg(feature = "gui")]
            indexes: HashMap::new(),
        }
    }

    /// Resolve `name` to a bitmap scaled to fit `self.size`, memoized.
    /// Core-only builds have no decoder: find() simply reports no icon.
    #[cfg(feature = "gui")]
    pub fn find(&mut self, name: &str) -> Option<Icon> {
        if name.is_empty() {
            return None;
        }
        if let Some(hit) = self.cache.get(name) {
            return hit.clone();
        }
        let resolved = self.resolve_uncached(name);
        self.cache.insert(name.to_string(), resolved.clone());
        resolved
    }

    #[cfg(not(feature = "gui"))]
    #[allow(dead_code)] // cell_icon (the caller) is gui-only
    pub fn find(&mut self, _name: &str) -> Option<Icon> {
        None
    }

    #[cfg(feature = "gui")]
    fn resolve_uncached(&mut self, name: &str) -> Option<Icon> {
        // Absolute path in Icon= is used as-is.
        if name.starts_with('/') {
            return load_file(Path::new(name), self.size);
        }

        // Owned list of every theme dir in search order, so the index build
        // (which needs &mut self) can run while we iterate.
        let mut dirs = Vec::new();
        for theme in &self.themes {
            for base in &self.bases {
                dirs.push(base.join("icons").join(theme));
            }
        }

        let mut xpm_seen = false;
        for dir in &dirs {
            let (path, xpm) = self.lookup_in(dir, name);
            xpm_seen |= xpm;
            if let Some(path) = path {
                return load_file(&path, self.size);
            }
        }

        // Legacy /usr/share/pixmaps/<name>.png|.svg
        for base in &self.bases {
            for ext in ["png", "svg"] {
                let p = base.join("pixmaps").join(format!("{name}.{ext}"));
                if p.is_file() {
                    return load_file(&p, self.size);
                }
            }
            if base.join("pixmaps").join(format!("{name}.xpm")).is_file() {
                xpm_seen = true;
            }
        }

        if xpm_seen {
            warn_xpm_only(name);
        }
        None
    }

    /// Look `name` up in one theme directory via its cached index.
    /// Returns the best PNG/SVG candidate path (if this directory has one)
    /// and whether an XPM twin exists here (for the XPM-only warning).
    #[cfg(feature = "gui")]
    fn lookup_in(&mut self, theme_dir: &Path, name: &str) -> (Option<PathBuf>, bool) {
        if !self.indexes.contains_key(theme_dir) {
            let index = index_theme(theme_dir, self.size);
            self.indexes.insert(theme_dir.to_path_buf(), index);
        }
        let index = self.indexes.get(theme_dir).expect("index inserted above");
        (
            index.best.get(name).map(|(_, path)| path.clone()),
            index.xpm.contains(name),
        )
    }
}

/// The theme search chain: `theme` itself, then everything its
/// `index.theme` `Inherits=` pulls in (depth-first, with a visited set so
/// inheritance cycles cannot loop forever), and `hicolor` last — the
/// spec's implicit final fallback.
fn theme_chain(bases: &[PathBuf], theme: &str) -> Vec<String> {
    fn push(bases: &[PathBuf], name: &str, chain: &mut Vec<String>, visited: &mut HashSet<String>) {
        if !visited.insert(name.to_string()) {
            return; // already in the chain, or a cycle
        }
        chain.push(name.to_string());
        // the first base dir that carries an index.theme for this theme wins
        for base in bases {
            let parents = theme_inherits(base, name);
            if !parents.is_empty() {
                for parent in parents {
                    push(bases, &parent, chain, visited);
                }
                return;
            }
        }
    }

    let mut chain = Vec::new();
    let mut visited = HashSet::new();
    push(bases, theme, &mut chain, &mut visited);
    if visited.insert("hicolor".to_string()) {
        chain.push("hicolor".to_string());
    }
    chain
}

/// `Inherits=` of one theme's index.theme (first `base/icons/<theme>/index.theme`
/// that exists), parent names in declared order; empty when there is none.
fn theme_inherits(base: &Path, theme: &str) -> Vec<String> {
    let index = base.join("icons").join(theme).join("index.theme");
    let Ok(text) = std::fs::read_to_string(&index) else {
        return Vec::new();
    };
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.trim_matches(|c| c == '[' || c == ']').to_string();
            continue;
        }
        if section != "Icon Theme" {
            continue;
        }
        if let Some(value) = line
            .strip_prefix("Inherits")
            .and_then(|v| v.strip_prefix('='))
        {
            return value
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    Vec::new()
}

/// Walk one theme directory **once** and index the best candidate for every
/// name in it, using the same ranking the per-name walk used before:
/// 0 = exact-size PNG, 1 = any SVG (scales), 8+distance = other PNG.
/// XPM files are recorded separately (never decoded — see warn_xpm_only).
/// The index lives for the whole process; icons are not added to a theme
/// while a launcher session runs.
#[cfg(feature = "gui")]
fn index_theme(theme_dir: &Path, want: u32) -> ThemeIndex {
    let mut index = ThemeIndex::default();
    walk(theme_dir, &mut |path| {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let score = match ext.as_str() {
            "png" => {
                let size = dir_size(path.parent());
                if size == want {
                    0
                } else {
                    8 + size.abs_diff(want).min(4096)
                }
            }
            "svg" => 1,
            "xpm" => {
                index.xpm.insert(stem.to_string());
                return;
            }
            _ => return,
        };
        let better = match index.best.get(stem) {
            Some((prev, _)) => score < *prev,
            None => true,
        };
        // strictly-better wins; equal scores keep the first-seen file
        // (the same tie rule the per-name walk had)
        if better {
            index
                .best
                .insert(stem.to_string(), (score, path.to_path_buf()));
        }
    });
    index
}

/// Warned-once flag: an XPM-only icon produces at most one stderr note per
/// process, no matter how many legacy apps lack modern icons.
#[cfg(feature = "gui")]
static XPM_WARNING_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "gui")]
fn warn_xpm_only(name: &str) {
    use std::sync::atomic::Ordering;
    if !XPM_WARNING_SHOWN.swap(true, Ordering::Relaxed) {
        eprintln!(
            "foto: icon {name:?} only exists as an XPM — XPM is not decoded (legacy \
             format); consider a theme that ships PNG/SVG icons"
        );
    }
}

/// Recursively visit every file under `dir`.
#[cfg(feature = "gui")]
fn walk(dir: &Path, visit: &mut impl FnMut(&Path)) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut any = false;
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            any |= walk(&path, visit);
        } else {
            visit(&path);
            any = true;
        }
    }
    any
}

/// "48x48" or "48" directory names carry the icon size; "scalable" -> 0.
/// The file usually sits in a context subdirectory (`48x48/apps/foo.png`),
/// so if the immediate parent has no size we also try the grandparent —
/// otherwise every icon would look like size 0 and "closest size" would
/// never work on standard theme layouts.
fn dir_size(dir: Option<&Path>) -> u32 {
    let mut cur = dir;
    for _ in 0..2 {
        let Some(d) = cur else { return 0 };
        if let Some(name) = d.file_name().and_then(|n| n.to_str()) {
            let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(size) = digits.parse::<u32>() {
                if size > 0 {
                    return size;
                }
            }
        }
        cur = d.parent();
    }
    0
}

/// Decode + scale any raster or SVG file into an `Icon` that fits `size`.
#[cfg(feature = "gui")]
pub(crate) fn load_file(path: &Path, size: u32) -> Option<Icon> {
    if path.extension()?.to_str()?.eq_ignore_ascii_case("svg") {
        load_svg(path, size)
    } else {
        load_raster(path, size)
    }
}

#[cfg(feature = "gui")]
fn load_raster(path: &Path, size: u32) -> Option<Icon> {
    let img = image::open(path).ok()?;
    // thumbnail() uses fast integer sampling — a big win over resize()'s
    // two-pass filter when shrinking 6000x4000 wallpapers; small icons are
    // left at their natural size (no blur from upscaling).
    let thumb = img.thumbnail(size, size);
    let mut rgba = thumb.to_rgba8().into_raw();
    premultiply(&mut rgba);
    Some(Icon {
        w: thumb.width(),
        h: thumb.height(),
        rgba,
    })
}

#[cfg(feature = "gui")]
fn load_svg(path: &Path, size: u32) -> Option<Icon> {
    let data = std::fs::read(path).ok()?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;

    let mut pm = tiny_skia::Pixmap::new(size, size)?;
    // Scale the SVG's natural size to fill the requested box, preserving aspect.
    let natural = tree.size();
    let scale = (size as f32 / natural.width())
        .min(size as f32 / natural.height())
        .min(100.0); // sanity clamp for tiny/zero-sized SVGs
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pm.as_mut());
    Some(Icon {
        w: size,
        h: size,
        rgba: pm.data().to_vec(), // resvg output is already premultiplied
    })
}

/// Convert straight-alpha RGBA to premultiplied in place.
#[cfg(feature = "gui")]
pub(crate) fn premultiply(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = px[3] as u32;
        px[0] = ((px[0] as u32 * a) / 255) as u8;
        px[1] = ((px[1] as u32 * a) / 255) as u8;
        px[2] = ((px[2] as u32 * a) / 255) as u8;
    }
}

/// Data dirs an icon theme may live under (shared helper from desktop.rs).
fn default_bases() -> Vec<PathBuf> {
    crate::desktop::data_dirs()
}

#[cfg(all(test, feature = "gui"))]
mod tests {
    use super::*;

    fn png_bytes(color: [u8; 3], w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |_, _| image::Rgb(color));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    /// Build a temp icon theme: base/icons/hicolor/{48x48/apps,32x32/apps}/<name>.png
    fn temp_theme(tag: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("foto-icons-{tag}"));
        // a previous test run may have left files behind — start clean
        let _ = std::fs::remove_dir_all(&base);
        let apps48 = base.join("icons/hicolor/48x48/apps");
        let apps32 = base.join("icons/hicolor/32x32/apps");
        std::fs::create_dir_all(&apps48).unwrap();
        std::fs::create_dir_all(&apps32).unwrap();
        (base, apps48)
    }

    #[test]
    fn finds_png_and_prefers_exact_size() {
        let (base, apps48) = temp_theme("exact");
        std::fs::write(apps48.join("testapp.png"), png_bytes([255, 0, 0], 48, 48)).unwrap();

        let mut finder = IconFinder::with_bases(vec![base], "hicolor", 48);
        let icon = finder.find("testapp").expect("icon should resolve");
        assert_eq!((icon.w, icon.h), (48, 48));
        assert!(
            finder.find("testapp").is_some(),
            "second call should hit cache"
        );
        assert!(finder.find("no-such-icon").is_none());
    }

    #[test]
    fn scales_when_no_exact_size() {
        let (base, apps48) = temp_theme("scale");
        std::fs::write(apps48.join("only48.png"), png_bytes([0, 255, 0], 48, 48)).unwrap();

        let mut finder = IconFinder::with_bases(vec![base], "hicolor", 32);
        let icon = finder.find("only48").expect("fallback size should work");
        assert_eq!((icon.w, icon.h), (32, 32));
    }

    #[test]
    fn absolute_path_icons() {
        let p = std::env::temp_dir().join("foto-abs-icon.png");
        std::fs::write(&p, png_bytes([0, 0, 255], 16, 16)).unwrap();
        let mut finder = IconFinder::with_bases(vec![], "hicolor", 16);
        let icon = finder.find(p.to_str().unwrap()).expect("absolute path");
        assert_eq!(icon.w, 16);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn colors_are_premultiplied() {
        // 2x1 image: one fully opaque red pixel, one transparent red pixel.
        let mut img = image::RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([255, 0, 0, 0]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let p = std::env::temp_dir().join("foto-premult-test.png");
        std::fs::write(&p, buf.into_inner()).unwrap();

        let icon = load_file(&p, 2).unwrap();
        assert_eq!(icon.rgba[0], 255, "opaque pixel unchanged");
        assert_eq!(icon.rgba[4], 0, "transparent pixel premultiplied to 0");
        let _ = std::fs::remove_file(&p);
    }

    fn svg_bytes(w: u32, h: u32) -> Vec<u8> {
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\">\
             <rect width=\"{w}\" height=\"{h}\" fill=\"#00ff00\"/></svg>"
        )
        .into_bytes()
    }

    #[test]
    fn closest_directory_size_wins() {
        // want 32: the 24px directory (distance 8) must beat the 48px one
        // (distance 16). The score is directory-based, and the winner is
        // revealed by its COLOR (red = 24px dir, blue = 48px dir), not by
        // the bitmap size — image::thumbnail upscales small sources to the
        // requested size.
        let (base, apps48) = temp_theme("closest");
        let apps24 = base.join("icons/hicolor/24x24/apps");
        std::fs::create_dir_all(&apps24).unwrap();
        std::fs::write(apps24.join("two.png"), png_bytes([255, 0, 0], 24, 24)).unwrap();
        std::fs::write(apps48.join("two.png"), png_bytes([0, 0, 255], 48, 48)).unwrap();

        let mut finder = IconFinder::with_bases(vec![base], "hicolor", 32);
        let icon = finder.find("two").expect("icon must resolve");
        assert_eq!(
            icon.rgba[0], 255,
            "24px dir (red) is closer to 32 than 48px (blue)"
        );
        assert_eq!(icon.rgba[2], 0);
    }

    #[test]
    fn exact_png_beats_svg_and_svg_beats_wrong_size_png() {
        // case 1: an exact-size PNG beats the always-available SVG (score 0 < 1)
        let (base, apps48) = temp_theme("pngvssvg");
        std::fs::write(apps48.join("both.png"), png_bytes([255, 0, 0], 48, 48)).unwrap();
        std::fs::write(apps48.join("both.svg"), svg_bytes(48, 48)).unwrap();
        let mut finder = IconFinder::with_bases(vec![base], "hicolor", 48);
        let icon = finder.find("both").expect("icon must resolve");
        assert_eq!(icon.rgba[0], 255, "exact-size PNG wins over SVG");
        assert_eq!(icon.rgba[1], 0, "PNG is red, SVG would be green");

        // case 2: with no exact-size directory the SVG (score 1) beats a
        // PNG in a wrong-size directory (score 8 + distance) — it scales
        // cleanly. The PNG lives in 32x32/ so the 48x48 request misses it.
        let (base2, apps48b) = temp_theme("svgfallback");
        let apps32b = base2.join("icons/hicolor/32x32/apps");
        std::fs::create_dir_all(&apps32b).unwrap();
        std::fs::write(apps32b.join("mixed.png"), png_bytes([255, 0, 0], 32, 32)).unwrap();
        std::fs::write(apps48b.join("mixed.svg"), svg_bytes(48, 48)).unwrap();
        let mut finder2 = IconFinder::with_bases(vec![base2], "hicolor", 48);
        let icon2 = finder2.find("mixed").expect("icon must resolve");
        assert_eq!(
            (icon2.w, icon2.h),
            (48, 48),
            "SVG renders at the requested size"
        );
        assert_eq!(icon2.rgba[1], 255, "SVG is green");
    }

    #[test]
    fn falls_back_to_hicolor_theme() {
        // the requested theme has no icon; hicolor (always appended to the
        // theme chain) must provide it
        let (base, apps48) = temp_theme("hifallback");
        std::fs::create_dir_all(base.join("icons/Papirus")).unwrap(); // empty theme
        std::fs::write(apps48.join("shared.png"), png_bytes([0, 255, 0], 48, 48)).unwrap();

        let mut finder = IconFinder::with_bases(vec![base], "Papirus", 48);
        let icon = finder
            .find("shared")
            .expect("hicolor fallback must resolve");
        assert_eq!((icon.w, icon.h), (48, 48));
        assert!(finder.find("still-missing").is_none());
    }

    #[test]
    fn follows_inherits_chain() {
        // Papirus inherits Papirus-Dark, which has the icon; Papirus itself
        // has none. The chain must reach Papirus-Dark.
        let (base, _apps48) = temp_theme("inherits");
        let dark = base.join("icons/Papirus-Dark/48x48/apps");
        std::fs::create_dir_all(&dark).unwrap();
        std::fs::create_dir_all(base.join("icons/Papirus/48x48/apps")).unwrap();
        std::fs::write(
            base.join("icons/Papirus/index.theme"),
            "[Icon Theme]\nName=Papirus\nInherits=Papirus-Dark\n",
        )
        .unwrap();
        std::fs::write(dark.join("chained.png"), png_bytes([255, 0, 255], 48, 48)).unwrap();

        let mut finder = IconFinder::with_bases(vec![base], "Papirus", 48);
        let icon = finder
            .find("chained")
            .expect("inherited theme must resolve");
        assert_eq!((icon.w, icon.h), (48, 48));
    }

    #[test]
    fn inherits_is_depth_first_in_declared_order() {
        // A inherits B and C; BOTH have the icon. Depth-first means B (the
        // first declared parent, including B's own chain) wins over C.
        let (base, _apps48) = temp_theme("dfs");
        let b = base.join("icons/B/48x48/apps");
        let c = base.join("icons/C/48x48/apps");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(&c).unwrap();
        std::fs::create_dir_all(base.join("icons/A")).unwrap();
        std::fs::write(
            base.join("icons/A/index.theme"),
            "[Icon Theme]\nInherits=B,C\n",
        )
        .unwrap();
        std::fs::write(b.join("only.png"), png_bytes([255, 0, 0], 48, 48)).unwrap();
        std::fs::write(c.join("only.png"), png_bytes([0, 0, 255], 48, 48)).unwrap();

        let mut finder = IconFinder::with_bases(vec![base], "A", 48);
        let icon = finder.find("only").expect("icon must resolve");
        assert_eq!(icon.rgba[0], 255, "first declared parent (B, red) wins");
    }

    #[test]
    fn inherits_cycle_does_not_loop_forever() {
        // A -> B -> A ... the visited set must cut the cycle and still find
        // the icon that lives in B.
        let (base, _apps48) = temp_theme("cycle");
        let b = base.join("icons/B/48x48/apps");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(base.join("icons/A")).unwrap();
        std::fs::write(
            base.join("icons/A/index.theme"),
            "[Icon Theme]\nInherits=B\n",
        )
        .unwrap();
        std::fs::write(
            base.join("icons/B/index.theme"),
            "[Icon Theme]\nInherits=A\n",
        )
        .unwrap();
        std::fs::write(b.join("looped.png"), png_bytes([0, 255, 255], 48, 48)).unwrap();

        let mut finder = IconFinder::with_bases(vec![base], "A", 48);
        let icon = finder
            .find("looped")
            .expect("cycle must not prevent lookup");
        assert_eq!((icon.w, icon.h), (48, 48));
    }

    #[test]
    fn explicit_hicolor_inherit_is_not_duplicated() {
        // A inherits hicolor explicitly: the chain must contain hicolor
        // exactly once, and the lookup still works.
        let (base, apps48) = temp_theme("exphicolor");
        std::fs::create_dir_all(base.join("icons/A")).unwrap();
        std::fs::write(
            base.join("icons/A/index.theme"),
            "[Icon Theme]\nInherits=hicolor\n",
        )
        .unwrap();
        std::fs::write(apps48.join("shared.png"), png_bytes([9, 9, 9], 48, 48)).unwrap();

        let mut finder = IconFinder::with_bases(vec![base], "A", 48);
        assert!(finder.find("shared").is_some());
    }

    #[test]
    fn falls_back_to_pixmaps() {
        let base = std::env::temp_dir().join("foto-icons-pixmaps");
        let pix = base.join("pixmaps");
        std::fs::create_dir_all(&pix).unwrap();
        std::fs::write(pix.join("legacy.png"), png_bytes([255, 255, 0], 32, 32)).unwrap();

        let mut finder = IconFinder::with_bases(vec![base.clone()], "hicolor", 32);
        let icon = finder
            .find("legacy")
            .expect("pixmaps fallback must resolve");
        assert_eq!((icon.w, icon.h), (32, 32));
        assert!(finder.find("nope").is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn xpm_only_icon_resolves_to_none() {
        // XPM is not decoded: the finder must return None (not panic) and
        // prefer any non-XPM alternative when one exists
        let (base, apps48) = temp_theme("xpm");
        std::fs::create_dir_all(base.join("icons/hicolor/48x48/apps")).unwrap();
        std::fs::write(apps48.join("legacy.xpm"), b"/* XPM */ static char*x=[];").unwrap();
        std::fs::write(apps48.join("both.xpm"), b"/* XPM */").unwrap();
        std::fs::write(apps48.join("both.png"), png_bytes([1, 2, 3], 48, 48)).unwrap();

        let mut finder = IconFinder::with_bases(vec![base], "hicolor", 48);
        assert!(finder.find("legacy").is_none(), "xpm-only must not resolve");
        // the PNG sibling wins over the XPM
        assert!(finder.find("both").is_some());
    }

    #[test]
    fn index_serves_every_name_from_one_walk() {
        // 12 names spread over two size dirs: after the first lookup builds
        // the index, every other name must resolve too — one walk, many hits
        // (this is the regression test for the old walk-per-icon scheme)
        let (base, apps48) = temp_theme("index-many");
        let apps32 = base.join("icons/hicolor/32x32/apps");
        std::fs::create_dir_all(&apps32).unwrap();
        for i in 0..12 {
            let dir = if i % 2 == 0 { &apps48 } else { &apps32 };
            std::fs::write(
                dir.join(format!("app{i}.png")),
                png_bytes([i as u8, 64, 0], 48, 48),
            )
            .unwrap();
        }
        let mut finder = IconFinder::with_bases(vec![base], "hicolor", 48);
        for i in 0..12 {
            assert!(
                finder.find(&format!("app{i}")).is_some(),
                "app{i} must resolve from the shared index"
            );
        }
        assert!(finder.find("missing").is_none());
    }
}
