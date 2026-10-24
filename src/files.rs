//! `foto --files`: scanning a directory for image files.

use crate::icons::Icon;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ImageFile {
    pub path: PathBuf,
}

impl ImageFile {
    pub fn name(&self) -> &str {
        self.path.file_name().and_then(|n| n.to_str()).unwrap_or("")
    }
}

const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];

/// How deep a recursive `--files` walk may go (depth 0 = the top directory
/// itself). Keeps a stray symlink loop or a huge tree from hanging startup.
pub const MAX_SCAN_DEPTH: usize = 8;

pub fn default_dir() -> Option<PathBuf> {
    // XDG pictures dir (from ~/.config/user-dirs.dirs); fall back to a plain
    // ~/Pictures for systems without xdg-user-dirs configured. Returns None
    // only when neither exists, so the caller's error message stays honest.
    dirs::picture_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Pictures")))
        .filter(|p| p.is_dir())
}

/// Non-recursive scan of one directory, sorted by name.
pub fn scan(dir: &Path) -> Vec<ImageFile> {
    let mut out = Vec::new();
    scan_streaming(dir, false, &mut |batch| out.extend(batch.iter().cloned()));
    out
}

/// Walk `dir`, handing every discovered directory's image batch to
/// `on_batch` as soon as it is found — streaming, so callers can start
/// working with the first results before the whole tree has been walked.
///
/// With `recursive` the walk descends into subdirectories (depth-limited to
/// [`MAX_SCAN_DEPTH`], dot-directories are skipped); otherwise only the top
/// directory is scanned. Returns the total number of images found.
pub fn scan_streaming(
    dir: &Path,
    recursive: bool,
    on_batch: &mut dyn FnMut(&[ImageFile]),
) -> usize {
    walk_dir(dir, recursive, 0, on_batch)
}

fn walk_dir(
    dir: &Path,
    recursive: bool,
    depth: usize,
    on_batch: &mut dyn FnMut(&[ImageFile]),
) -> usize {
    let Ok(read) = std::fs::read_dir(dir) else {
        return 0; // missing/unreadable directory is normal, not an error
    };
    let mut batch = Vec::new();
    let mut subdirs = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive
                && depth < MAX_SCAN_DEPTH
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| !n.starts_with('.'))
            {
                subdirs.push(path);
            }
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if EXTENSIONS.iter().any(|x| ext.eq_ignore_ascii_case(x)) {
                batch.push(ImageFile { path });
            }
        }
    }
    batch.sort_by_key(|a| a.name().to_lowercase());
    let mut total = batch.len();
    if !batch.is_empty() {
        on_batch(&batch);
    }
    if recursive {
        for sub in subdirs {
            total += walk_dir(&sub, recursive, depth + 1, on_batch);
        }
    }
    total
}

/// Decode an image and shrink it to a thumbnail that fits `size` (cached by callers).
#[cfg(feature = "gui")]
pub fn load_thumbnail(path: &Path, size: u32) -> Option<Icon> {
    // on-disk cache first: a 6000x4000 wallpaper decodes in ~200-400ms,
    // which made every launch of --files painful. After the first browse,
    // thumbnails come back instantly.
    if let Some(hit) = thumb_cache::load(path, size) {
        return Some(hit);
    }
    let icon = super::icons::load_file(path, size)?;
    thumb_cache::store(path, size, &icon);
    Some(icon)
}

/// On-disk thumbnail cache under `$XDG_CACHE_HOME/foto/thumbs`.
/// File name = hash of (path, size, mtime, len); contents = tiny header +
/// raw premultiplied RGBA, so a cache hit is a single sequential read.
/// Clear it with `rm -rf ~/.cache/foto` if anything ever looks stale.
pub(crate) mod thumb_cache {
    use super::Icon;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::path::{Path, PathBuf};

    // store() is only exercised by the GUI decode path
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    const MAGIC: &[u8; 5] = b"FOTO1";
    const HEADER: usize = 13; // magic(5) + width(4) + height(4)

    fn cache_dir() -> Option<PathBuf> {
        dirs::cache_dir().map(|d| d.join("foto").join("thumbs"))
    }

    /// Cache file for (source path, requested size). None if the source
    /// vanished or has no mtime (then we just skip caching).
    fn key_for(path: &Path, size: u32) -> Option<PathBuf> {
        let meta = std::fs::metadata(path).ok()?;
        let mut h = DefaultHasher::new();
        path.to_string_lossy().hash(&mut h);
        meta.modified().ok()?.hash(&mut h);
        meta.len().hash(&mut h);
        size.hash(&mut h);
        Some(cache_dir()?.join(format!("{:016x}.bin", h.finish())))
    }

    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub fn load(path: &Path, size: u32) -> Option<Icon> {
        let file = std::fs::read(key_for(path, size)?).ok()?;
        if file.len() < HEADER || &file[0..5] != MAGIC {
            return None;
        }
        let w = u32::from_be_bytes(file[5..9].try_into().ok()?) as usize;
        let h = u32::from_be_bytes(file[9..13].try_into().ok()?) as usize;
        if w == 0 || h == 0 || file.len() != HEADER + w * h * 4 {
            return None; // truncated/corrupt -> treat as a miss
        }
        Some(Icon {
            w: w as u32,
            h: h as u32,
            rgba: file[HEADER..].to_vec(),
        })
    }

    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub fn store(path: &Path, size: u32, icon: &Icon) {
        let Some(key) = key_for(path, size) else {
            return;
        };
        let mut out = Vec::with_capacity(HEADER + icon.rgba.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&icon.w.to_be_bytes());
        out.extend_from_slice(&icon.h.to_be_bytes());
        out.extend_from_slice(&icon.rgba);
        if let Some(dir) = key.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // write-then-rename so a crash mid-write never leaves a bad entry
        let tmp = key.with_extension("tmp");
        if std::fs::write(&tmp, &out).is_ok() {
            let _ = std::fs::rename(&tmp, &key);
        }
    }
}

#[cfg(all(test, feature = "gui"))]
mod tests {
    use super::*;

    #[test]
    fn thumb_cache_roundtrip() {
        let dir = std::env::temp_dir().join(format!("foto-thumbcache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("wall.png");
        std::fs::write(&src, b"fake").unwrap();

        let icon = Icon {
            w: 3,
            h: 2,
            rgba: vec![9; 3 * 2 * 4],
        };
        thumb_cache::store(&src, 96, &icon);

        let hit = thumb_cache::load(&src, 96).expect("cache hit");
        assert_eq!((hit.w, hit.h), (3, 2));
        assert_eq!(hit.rgba, icon.rgba);

        // different size -> different key -> miss
        assert!(thumb_cache::load(&src, 24).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scans_only_images_sorted() {
        let dir = std::env::temp_dir().join(format!("foto-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.png"), b"x").unwrap();
        std::fs::write(dir.join("a.PNG"), b"x").unwrap(); // case-insensitive
        std::fs::write(dir.join("c.txt"), b"x").unwrap(); // not an image
        std::fs::create_dir(dir.join("sub.png")).unwrap(); // dirs don't count

        let files = scan(&dir);
        let names: Vec<&str> = files.iter().map(|f| f.name()).collect();
        assert_eq!(names, vec!["a.PNG", "b.png"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_is_not_recursive() {
        let dir = std::env::temp_dir().join(format!("foto-files-nr-{}", std::process::id()));
        let sub = dir.join("subdir");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.join("top.png"), b"x").unwrap();
        std::fs::write(sub.join("nested.jpg"), b"x").unwrap();

        let files = scan(&dir);
        let names: Vec<&str> = files.iter().map(|f| f.name()).collect();
        assert_eq!(
            names,
            vec!["top.png"],
            "subdirectory images must be skipped"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nonexistent_dir_yields_empty_not_panic() {
        let missing = std::env::temp_dir().join(format!(
            "foto-files-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!missing.exists());
        assert!(scan(&missing).is_empty());
        // and it leaves nothing behind
        assert!(!missing.exists());
    }

    #[test]
    fn recursive_scan_finds_nested_images() {
        let dir = std::env::temp_dir().join(format!("foto-files-rec-{}", std::process::id()));
        let deep = dir.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(dir.join("top.png"), b"x").unwrap();
        std::fs::write(dir.join("a/mid.jpg"), b"x").unwrap();
        std::fs::write(deep.join("bottom.webp"), b"x").unwrap();
        std::fs::write(deep.join("notes.txt"), b"x").unwrap(); // not an image

        let mut names = Vec::new();
        let total = scan_streaming(&dir, true, &mut |batch| {
            for f in batch {
                names.push(f.name().to_string());
            }
        });
        assert_eq!(total, 3);
        // batches arrive per directory, each sorted; walk is depth-first:
        // top-level first, then a/, then a/b/c/
        assert_eq!(names, vec!["top.png", "mid.jpg", "bottom.webp"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recursive_scan_skips_dot_directories_and_respects_depth_cap() {
        let dir = std::env::temp_dir().join(format!("foto-files-depth-{}", std::process::id()));
        // a dot-directory with an image inside must be skipped entirely
        let hidden = dir.join(".hidden");
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(hidden.join("no.png"), b"x").unwrap();
        // a chain deeper than MAX_SCAN_DEPTH must be cut off
        let mut deep = dir.clone();
        for i in 0..MAX_SCAN_DEPTH + 3 {
            deep = deep.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("too-deep.png"), b"x").unwrap();
        // one just inside the limit must be found
        let mut inside = dir.clone();
        for i in 0..MAX_SCAN_DEPTH {
            inside = inside.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(inside.join("just-inside.png"), b"x").unwrap();

        let mut names = Vec::new();
        scan_streaming(&dir, true, &mut |batch| {
            for f in batch {
                names.push(f.name().to_string());
            }
        });
        assert_eq!(names, vec!["just-inside.png"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recursive_flag_off_keeps_top_level_only() {
        let dir = std::env::temp_dir().join(format!("foto-files-nr2-{}", std::process::id()));
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.join("top.png"), b"x").unwrap();
        std::fs::write(sub.join("nested.png"), b"x").unwrap();

        let mut batches = 0;
        let total = scan_streaming(&dir, false, &mut |batch| {
            batches += 1;
            assert_eq!(batch.len(), 1);
        });
        assert_eq!(total, 1, "recursive=false must not descend");
        assert_eq!(batches, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
