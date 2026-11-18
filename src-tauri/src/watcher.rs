use crate::audio::is_supported_extension;
use crate::queue::QueueManager;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct CandidateState {
    size: u64,
    stable_since: Instant,
    stable_ticks: u32,
}

#[derive(Clone)]
pub struct FolderWatcher {
    queue: QueueManager,
    inner: Arc<Mutex<WatcherInner>>,
}

struct WatcherInner {
    active_dir: Option<PathBuf>,
    _notify_watcher: Option<RecommendedWatcher>,
    candidates: HashMap<PathBuf, CandidateState>,
    reported_access_error: Option<String>,
    dirty: bool,
}

fn is_ignored_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return true;
    };
    if name.starts_with('.') {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".part")
        || lower.ends_with(".tmp")
        || lower.ends_with(".crdownload")
        || lower.ends_with(".download")
        || lower.ends_with(".partial")
        || lower.ends_with(".txt")
        || lower.ends_with(".srt")
        || lower.ends_with(".vtt")
}

impl FolderWatcher {
    pub fn new(queue: QueueManager) -> Self {
        let watcher = Self {
            queue,
            inner: Arc::new(Mutex::new(WatcherInner {
                active_dir: None,
                _notify_watcher: None,
                candidates: HashMap::new(),
                reported_access_error: None,
                dirty: false,
            })),
        };

        // Restore persisted watch folder if active
        let (saved_folder, active) = watcher.queue.watch_config();
        if active {
            if let Some(folder) = saved_folder {
                let _ = watcher.start_watching(Path::new(&folder));
            }
        }

        watcher.spawn_poll_loop();
        watcher
    }

    pub fn start_watching(&self, dir: &Path) -> Result<(), String> {
        let dir_str = dir.to_string_lossy().to_string();
        if !dir.exists() {
            let msg = format!("Watch folder does not exist: '{}'", dir_str);
            self.queue.record_error(&dir_str, "WATCH_FOLDER_NOT_FOUND", &msg);
            return Err(msg);
        }
        if !dir.is_dir() {
            let msg = format!("Watch path is not a directory: '{}'", dir_str);
            self.queue.record_error(&dir_str, "WATCH_FOLDER_INVALID", &msg);
            return Err(msg);
        }
        if let Err(e) = fs::read_dir(dir) {
            let code = if e.kind() == std::io::ErrorKind::PermissionDenied {
                "PERMISSION_DENIED"
            } else {
                "WATCH_FOLDER_ACCESS_ERROR"
            };
            let msg = format!("Cannot access watch folder '{}': {}", dir_str, e);
            self.queue.record_error(&dir_str, code, &msg);
            return Err(msg);
        }

        let inner_clone = Arc::clone(&self.inner);
        let notify_res = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if res.is_ok() {
                if let Ok(mut guard) = inner_clone.lock() {
                    guard.dirty = true;
                }
            }
        });

        let mut rec_watcher = match notify_res {
            Ok(w) => Some(w),
            Err(_) => None,
        };
        if let Some(ref mut w) = rec_watcher {
            let _ = w.watch(dir, RecursiveMode::NonRecursive);
        }

        let canon_dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let canon_str = canon_dir.to_string_lossy().to_string();

        {
            let mut guard = self.inner.lock().expect("watcher lock");
            guard.active_dir = Some(canon_dir);
            guard._notify_watcher = rec_watcher;
            guard.candidates.clear();
            guard.reported_access_error = None;
            guard.dirty = true;
        }

        self.queue.set_watch_folder(Some(canon_str), true);
        Ok(())
    }

    pub fn stop_watching(&self) {
        {
            let mut guard = self.inner.lock().expect("watcher lock");
            guard.active_dir = None;
            guard._notify_watcher = None;
            guard.candidates.clear();
            guard.reported_access_error = None;
        }
        let (existing, _) = self.queue.watch_config();
        self.queue.set_watch_folder(existing, false);
    }

    fn spawn_poll_loop(&self) {
        let watcher = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(350));
            watcher.scan_once();
        });
    }

    fn scan_once(&self) {
        let dir = {
            let guard = self.inner.lock().expect("watcher lock");
            guard.active_dir.clone()
        };
        let Some(dir_path) = dir else {
            return;
        };

        let entries = match fs::read_dir(&dir_path) {
            Ok(rd) => rd,
            Err(e) => {
                let code = if e.kind() == std::io::ErrorKind::PermissionDenied {
                    "PERMISSION_DENIED"
                } else {
                    "WATCH_FOLDER_ACCESS_ERROR"
                };
                let dir_str = dir_path.to_string_lossy().to_string();
                let msg = format!("Lost access to watch folder '{}': {}", dir_str, e);
                let mut should_report = false;
                {
                    let mut guard = self.inner.lock().expect("watcher lock");
                    if guard.reported_access_error.as_deref() != Some(code) {
                        guard.reported_access_error = Some(code.to_string());
                        should_report = true;
                    }
                }
                if should_report {
                    self.queue.record_error(&dir_str, code, &msg);
                }
                return;
            }
        };

        let mut seen_paths: HashSet<PathBuf> = HashSet::new();
        let mut ready_to_enqueue: Vec<PathBuf> = Vec::new();
        let now = Instant::now();

        let mut file_infos: Vec<(PathBuf, u64)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || is_ignored_name(&path) || !is_supported_extension(&path) {
                continue;
            }
            if self.queue.is_path_known_for_watch(&path) {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                seen_paths.insert(path.clone());
                file_infos.push((path, meta.len()));
            }
        }

        {
            let mut guard = self.inner.lock().expect("watcher lock");
            guard.reported_access_error = None;
            guard.dirty = false;
            guard.candidates.retain(|p, _| seen_paths.contains(p));

            for (path, size) in file_infos {
                if size == 0 {
                    guard.candidates.insert(
                        path,
                        CandidateState {
                            size: 0,
                            stable_since: now,
                            stable_ticks: 0,
                        },
                    );
                    continue;
                }

                match guard.candidates.get_mut(&path) {
                    Some(prev) => {
                        if prev.size == size {
                            prev.stable_ticks += 1;
                            if prev.stable_ticks >= 2
                                && prev.stable_since.elapsed() >= Duration::from_millis(700)
                                && File::open(&path).is_ok()
                            {
                                ready_to_enqueue.push(path.clone());
                            }
                        } else {
                            // Still copying; reset stability window
                            prev.size = size;
                            prev.stable_since = now;
                            prev.stable_ticks = 0;
                        }
                    }
                    None => {
                        guard.candidates.insert(
                            path,
                            CandidateState {
                                size,
                                stable_since: now,
                                stable_ticks: 0,
                            },
                        );
                    }
                }
            }

            for p in &ready_to_enqueue {
                guard.candidates.remove(p);
            }
        }

        for path in ready_to_enqueue {
            self.queue.enqueue_file(&path, "watch", false);
        }
    }
}
