//! Background thumbnail loading for `--files` mode.
//!
//! The first render of a wallpaper directory used to decode every visible
//! image synchronously in the draw loop — a 6000×4000 JPEG costs 200–400 ms,
//! so `foto --files` felt frozen on a cold cache. This engine moves that
//! work onto worker threads:
//!
//!   * [`ThumbEngine::lookup`] is called from the render thread per visible
//!     cell and never blocks on a decode — it returns [`ThumbState::Pending`]
//!     and queues the work instead.
//!   * worker threads decode, write the on-disk cache and publish into a
//!     shared in-memory cache, then ping the event loop through a calloop
//!     channel so the next frame picks the thumbnails up.
//!   * a cheap synchronous on-disk-cache check stays in `lookup`: a hit is
//!     one small read, and it keeps second+ launches flicker-free.

// Icon/thumbnail machinery is compiled in every build so the shared core
// stays one codebase, but it is exercised only by the GUI frontend; core
// (tui-only) builds therefore legitimately contain unreached items here.
#![cfg_attr(not(feature = "gui"), allow(dead_code))]
#[cfg(feature = "gui")]
use crate::icons;
use crate::icons::Icon;
use calloop::channel::Sender;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What the renderer should draw in the thumbnail slot.
#[derive(Clone)]
pub enum ThumbState {
    /// Final answer (None = undecodable or the file vanished).
    Ready(Option<Icon>),
    /// A worker is decoding it — draw a placeholder this frame.
    Pending,
}

/// Keep at most this many decoded thumbnails in memory (~a few MB).
const MEM_CACHE_CAP: usize = 512;

struct Shared {
    /// (path, size) -> decoded thumbnail; None = known-undecodable
    mem: HashMap<(PathBuf, u32), Option<Icon>>,
    /// insertion order for FIFO eviction of `mem`
    order: VecDeque<(PathBuf, u32)>,
    /// requests already queued, so we never enqueue the same cell twice
    pending: HashSet<(PathBuf, u32)>,
}

impl Shared {
    fn insert(&mut self, key: (PathBuf, u32), icon: Option<Icon>) {
        if self.mem.len() >= MEM_CACHE_CAP {
            if let Some(oldest) = self.order.pop_front() {
                self.mem.remove(&oldest);
            }
        }
        self.order.push_back(key.clone());
        self.mem.insert(key, icon);
    }
}

pub struct ThumbEngine {
    shared: Arc<Mutex<Shared>>,
    queue: Mutex<std::sync::mpsc::Sender<(PathBuf, u32)>>,
    /// Kept alive so worker threads never see a closed channel.
    _receiver: Arc<Mutex<std::sync::mpsc::Receiver<(PathBuf, u32)>>>,
}

impl ThumbEngine {
    /// Spawn `workers` decode threads. `ping` is a calloop channel sender;
    /// every time new thumbnails become available a `()` is sent so the GUI
    /// event loop can schedule a redraw.
    pub fn new(ping: Sender<()>, workers: usize) -> Self {
        let (req_tx, req_rx) = std::sync::mpsc::channel::<(PathBuf, u32)>();
        let req_rx = Arc::new(Mutex::new(req_rx));
        let shared = Arc::new(Mutex::new(Shared {
            mem: HashMap::new(),
            order: VecDeque::new(),
            pending: HashSet::new(),
        }));

        for _ in 0..workers.max(1) {
            let ping = ping.clone();
            let req_rx = Arc::clone(&req_rx);
            let shared = Arc::clone(&shared);
            // dropping the JoinHandle detaches: threads live as long as the engine
            std::thread::spawn(move || loop {
                let next = req_rx.lock().unwrap().recv();
                let Ok((path, size)) = next else { return };
                #[cfg(feature = "gui")]
                let icon = decode_cached(&path, size);
                #[cfg(not(feature = "gui"))]
                let icon = crate::files::thumb_cache::load(&path, size);
                let mut sh = shared.lock().unwrap();
                sh.insert((path.clone(), size), icon);
                sh.pending.remove(&(path, size));
                drop(sh);
                let _ = ping.send(()); // wake the event loop for a redraw
            });
        }

        Self {
            shared,
            queue: Mutex::new(req_tx),
            _receiver: req_rx,
        }
    }

    /// Called from the render thread; never blocks on a decode.
    pub fn lookup(&self, path: &Path, size: u32) -> ThumbState {
        let key = (path.to_path_buf(), size);
        let mut sh = self.shared.lock().unwrap();
        if let Some(hit) = sh.mem.get(&key) {
            return ThumbState::Ready(hit.clone());
        }
        // synchronous on-disk cache check: one small read, keeps relaunches
        // flicker-free without a worker round-trip
        if let Some(icon) = crate::files::thumb_cache::load(path, size) {
            sh.insert(key, Some(icon.clone()));
            return ThumbState::Ready(Some(icon));
        }
        if sh.pending.insert(key) {
            let _ = self.queue.lock().unwrap().send((path.to_path_buf(), size));
        }
        ThumbState::Pending
    }
}

/// Disk cache first, then a full decode + store. Runs on worker threads.
/// Core builds have no decoder; the engine is never constructed there, but
/// the worker code must still compile — it then only sees disk-cache hits.
#[cfg(feature = "gui")]
fn decode_cached(path: &Path, size: u32) -> Option<Icon> {
    if let Some(icon) = crate::files::thumb_cache::load(path, size) {
        return Some(icon);
    }
    let icon = icons::load_file(path, size)?;
    crate::files::thumb_cache::store(path, size, &icon);
    Some(icon)
}
