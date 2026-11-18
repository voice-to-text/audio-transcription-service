use crate::audio::{self, extension_lowercase, is_supported_extension};
use crate::export;
use crate::models::{
    AppStateSnapshot, ErrorLogEntry, PersistedState, ProcessedRecord, QueueItem, QueueStatus,
};
use crate::whisper;
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use tauri::{AppHandle, Emitter};

#[derive(Clone)]
pub struct QueueManager {
    inner: Arc<QueueInner>,
}

struct QueueInner {
    state: Mutex<PersistedState>,
    active_worker_id: Mutex<Option<String>>,
    notify: Condvar,
    state_file: PathBuf,
    app_handle: Mutex<Option<AppHandle>>,
}

pub fn resolve_state_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("AUDIO_STUDIO_STATE_DIR") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Audio Transcription Studio");
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("Audio Transcription Studio");
        }
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("audio-transcription-studio");
        }
    }

    std::env::temp_dir().join("audio-transcription-studio")
}

fn compute_sha256(path: &Path) -> Result<(String, u64), std::io::Error> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut reader = BufReader::with_capacity(65_536, file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65_536];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok((hex::encode(hasher.finalize()), size))
}

fn canonical_or_raw(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

impl QueueManager {
    pub fn new(state_dir: Option<PathBuf>) -> Self {
        let dir = state_dir.unwrap_or_else(resolve_state_dir);
        let _ = std::fs::create_dir_all(&dir);
        let state_file = dir.join("queue_state.json");

        let mut loaded: PersistedState = if state_file.exists() {
            match std::fs::read_to_string(&state_file) {
                Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
                Err(_) => PersistedState::default(),
            }
        } else {
            PersistedState::default()
        };

        // Crash / restart recovery: any in-flight item returns to Queued without losing work.
        let mut recovered = false;
        for item in &mut loaded.items {
            if matches!(
                item.status,
                QueueStatus::Decoding
                    | QueueStatus::Transcribing
                    | QueueStatus::Exporting
                    | QueueStatus::WaitingForCopy
            ) {
                item.status = QueueStatus::Queued;
                item.progress = 0;
                item.status_detail = Some("Resumed in queue after restart".to_string());
                recovered = true;
            }
        }

        let manager = Self {
            inner: Arc::new(QueueInner {
                state: Mutex::new(loaded),
                active_worker_id: Mutex::new(None),
                notify: Condvar::new(),
                state_file,
                app_handle: Mutex::new(None),
            }),
        };

        if recovered || !manager.inner.state_file.exists() {
            manager.persist();
        }

        manager
    }

    pub fn attach_app_handle(&self, handle: AppHandle) {
        if let Ok(mut guard) = self.inner.app_handle.lock() {
            *guard = Some(handle);
        }
    }

    pub fn start_worker(&self) {
        let mgr = self.clone();
        std::thread::spawn(move || {
            mgr.worker_loop();
        });
    }

    fn emit_update(&self) {
        if let Ok(guard) = self.inner.app_handle.lock() {
            if let Some(app) = guard.as_ref() {
                let _ = app.emit("state-updated", self.snapshot());
            }
        }
    }

    fn persist(&self) {
        let json_str = {
            let guard = self.inner.state.lock().expect("state lock");
            serde_json::to_string_pretty(&*guard).unwrap_or_else(|_| "{}".to_string())
        };
        let parent = self
            .inner
            .state_file
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let _ = std::fs::create_dir_all(parent);
        let tmp = parent.join(format!(".queue_state.{}.tmp", uuid::Uuid::new_v4()));
        if let Ok(mut f) = File::create(&tmp) {
            if f.write_all(json_str.as_bytes()).is_ok() {
                let _ = f.sync_all();
                drop(f);
                let _ = std::fs::rename(&tmp, &self.inner.state_file);
                return;
            }
        }
        let _ = std::fs::remove_file(&tmp);
    }

    pub fn snapshot(&self) -> AppStateSnapshot {
        let active_worker_id = self
            .inner
            .active_worker_id
            .lock()
            .ok()
            .and_then(|g| g.clone());
        let state = self.inner.state.lock().expect("state lock");
        AppStateSnapshot {
            items: state.items.clone(),
            error_log: state.error_log.clone(),
            watch_folder: state.watch_folder.clone(),
            watch_active: state.watch_active,
            offline_mode: state.offline_mode,
            processed_count: state.processed_hashes.len(),
            engine: whisper::engine_info(state.offline_mode),
            active_worker_id,
        }
    }

    pub fn is_path_known_for_watch(&self, path: &Path) -> bool {
        let canon = canonical_or_raw(path);
        let state = self.inner.state.lock().expect("state lock");
        if state.processed_paths.contains_key(&canon) {
            return true;
        }
        state.items.iter().any(|item| {
            item.file_path == canon
                && matches!(
                    item.status,
                    QueueStatus::WaitingForCopy
                        | QueueStatus::Queued
                        | QueueStatus::Decoding
                        | QueueStatus::Transcribing
                        | QueueStatus::Exporting
                        | QueueStatus::Completed
                        | QueueStatus::SkippedDuplicate
                )
        })
    }

    pub fn record_error(&self, file_path: &str, error_code: &str, message: &str) {
        {
            let mut state = self.inner.state.lock().expect("state lock");
            state.error_log.insert(
                0,
                ErrorLogEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    timestamp: Utc::now().to_rfc3339(),
                    file_path: file_path.to_string(),
                    error_code: error_code.to_string(),
                    message: message.to_string(),
                },
            );
            if state.error_log.len() > 250 {
                state.error_log.truncate(250);
            }
        }
        self.persist();
        self.emit_update();
    }

    pub fn set_watch_folder(&self, folder: Option<String>, active: bool) {
        {
            let mut state = self.inner.state.lock().expect("state lock");
            state.watch_folder = folder;
            state.watch_active = active;
        }
        self.persist();
        self.emit_update();
    }

    pub fn watch_config(&self) -> (Option<String>, bool) {
        let state = self.inner.state.lock().expect("state lock");
        (state.watch_folder.clone(), state.watch_active)
    }

    pub fn clear_errors(&self) {
        {
            let mut state = self.inner.state.lock().expect("state lock");
            state.error_log.clear();
        }
        self.persist();
        self.emit_update();
    }

    pub fn clear_completed(&self) {
        {
            let mut state = self.inner.state.lock().expect("state lock");
            state.items.retain(|i| {
                !matches!(
                    i.status,
                    QueueStatus::Completed | QueueStatus::SkippedDuplicate
                )
            });
        }
        self.persist();
        self.emit_update();
    }

    pub fn remove_item(&self, id: &str) {
        {
            let mut state = self.inner.state.lock().expect("state lock");
            state.items.retain(|i| {
                !(i.id == id
                    && !matches!(
                        i.status,
                        QueueStatus::Decoding | QueueStatus::Transcribing | QueueStatus::Exporting
                    ))
            });
        }
        self.persist();
        self.emit_update();
    }

    pub fn retry_item(&self, id: &str) -> Option<QueueItem> {
        let updated = {
            let mut state = self.inner.state.lock().expect("state lock");
            if let Some(item) = state.items.iter_mut().find(|i| i.id == id) {
                if matches!(item.status, QueueStatus::Failed) {
                    item.status = QueueStatus::Queued;
                    item.progress = 0;
                    item.status_detail = Some("Queued for retry".to_string());
                    item.error_code = None;
                    item.error_message = None;
                    Some(item.clone())
                } else {
                    Some(item.clone())
                }
            } else {
                None
            }
        };
        if updated.is_some() {
            self.persist();
            self.inner.notify.notify_all();
            self.emit_update();
        }
        updated
    }

    pub fn force_retranscribe(&self, id: &str) -> Option<QueueItem> {
        let updated = {
            let mut state = self.inner.state.lock().expect("state lock");
            if let Some(item) = state.items.iter_mut().find(|i| i.id == id) {
                if !matches!(
                    item.status,
                    QueueStatus::Decoding | QueueStatus::Transcribing | QueueStatus::Exporting
                ) {
                    item.status = QueueStatus::Queued;
                    item.progress = 0;
                    item.status_detail = Some("Queued for forced re-transcription".to_string());
                    item.error_code = None;
                    item.error_message = None;
                    item.completed_at = None;
                    Some(item.clone())
                } else {
                    Some(item.clone())
                }
            } else {
                None
            }
        };
        if updated.is_some() {
            self.persist();
            self.inner.notify.notify_all();
            self.emit_update();
        }
        updated
    }

    pub fn reset_state(&self) {
        {
            let mut state = self.inner.state.lock().expect("state lock");
            *state = PersistedState::default();
        }
        self.persist();
        self.emit_update();
    }

    pub fn enqueue_file(&self, raw_path: &Path, source: &str, force: bool) -> QueueItem {
        let canon_str = canonical_or_raw(raw_path);
        let canon_path = PathBuf::from(&canon_str);
        let file_name = canon_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let format = extension_lowercase(&canon_path);

        // If exact path is already active in queue, do not duplicate.
        {
            let state = self.inner.state.lock().expect("state lock");
            if let Some(existing) = state.items.iter().find(|i| {
                i.file_path == canon_str
                    && matches!(
                        i.status,
                        QueueStatus::WaitingForCopy
                            | QueueStatus::Queued
                            | QueueStatus::Decoding
                            | QueueStatus::Transcribing
                            | QueueStatus::Exporting
                    )
            }) {
                return existing.clone();
            }
        }

        // Unsupported file extension check
        if !is_supported_extension(&canon_path) {
            let msg = format!(
                "Unsupported file format '.{}'. Supported formats: MP3, WAV, M4A, MP4.",
                format
            );
            let item = QueueItem {
                id: uuid::Uuid::new_v4().to_string(),
                file_path: canon_str.clone(),
                file_name,
                format,
                file_size: std::fs::metadata(&canon_path).map(|m| m.len()).unwrap_or(0),
                sha256: None,
                duration_ms: None,
                status: QueueStatus::Failed,
                progress: 0,
                status_detail: Some(msg.clone()),
                detected_language: None,
                is_silent: false,
                segments: Vec::new(),
                txt_path: None,
                srt_path: None,
                vtt_path: None,
                error_code: Some("UNSUPPORTED_FORMAT".to_string()),
                error_message: Some(msg.clone()),
                source: source.to_string(),
                created_at: Utc::now().to_rfc3339(),
                completed_at: None,
            };
            self.push_item_and_error(item.clone(), "UNSUPPORTED_FORMAT", &msg);
            return item;
        }

        // Compute SHA-256 hash & file size (also validates readability / permission)
        let (sha256, file_size) = match compute_sha256(&canon_path) {
            Ok(v) => v,
            Err(e) => {
                let code = if e.kind() == std::io::ErrorKind::PermissionDenied {
                    "PERMISSION_DENIED"
                } else {
                    "FILE_READ_ERROR"
                };
                let msg = format!("Cannot read '{}': {}", canon_str, e);
                let item = QueueItem {
                    id: uuid::Uuid::new_v4().to_string(),
                    file_path: canon_str.clone(),
                    file_name,
                    format,
                    file_size: 0,
                    sha256: None,
                    duration_ms: None,
                    status: QueueStatus::Failed,
                    progress: 0,
                    status_detail: Some(msg.clone()),
                    detected_language: None,
                    is_silent: false,
                    segments: Vec::new(),
                    txt_path: None,
                    srt_path: None,
                    vtt_path: None,
                    error_code: Some(code.to_string()),
                    error_message: Some(msg.clone()),
                    source: source.to_string(),
                    created_at: Utc::now().to_rfc3339(),
                    completed_at: None,
                };
                self.push_item_and_error(item.clone(), code, &msg);
                return item;
            }
        };

        // Duplicate protection: skip if identical SHA-256 already completed and outputs exist
        if !force {
            let dup_record = {
                let state = self.inner.state.lock().expect("state lock");
                state.processed_hashes.get(&sha256).cloned()
            };
            if let Some(rec) = dup_record {
                let expected = export::expected_export_paths(&canon_path);
                let rec_outputs_exist = Path::new(&rec.txt_path).exists()
                    && Path::new(&rec.srt_path).exists()
                    && Path::new(&rec.vtt_path).exists();
                let same_path_outputs_exist = expected.txt_path.exists()
                    && expected.srt_path.exists()
                    && expected.vtt_path.exists();

                if rec_outputs_exist || same_path_outputs_exist {
                    let item = QueueItem {
                        id: uuid::Uuid::new_v4().to_string(),
                        file_path: canon_str.clone(),
                        file_name,
                        format,
                        file_size,
                        sha256: Some(sha256),
                        duration_ms: None,
                        status: QueueStatus::SkippedDuplicate,
                        progress: 100,
                        status_detail: Some(format!(
                            "Duplicate skipped (already processed from '{}')",
                            rec.file_path
                        )),
                        detected_language: Some(rec.detected_language),
                        is_silent: false,
                        segments: Vec::new(),
                        txt_path: Some(rec.txt_path),
                        srt_path: Some(rec.srt_path),
                        vtt_path: Some(rec.vtt_path),
                        error_code: None,
                        error_message: None,
                        source: source.to_string(),
                        created_at: Utc::now().to_rfc3339(),
                        completed_at: Some(Utc::now().to_rfc3339()),
                    };
                    {
                        let mut state = self.inner.state.lock().expect("state lock");
                        state.processed_paths.insert(canon_str, item.sha256.clone().unwrap_or_default());
                        state.items.push(item.clone());
                    }
                    self.persist();
                    self.emit_update();
                    return item;
                }
            }
        }

        let item = QueueItem {
            id: uuid::Uuid::new_v4().to_string(),
            file_path: canon_str,
            file_name,
            format,
            file_size,
            sha256: Some(sha256),
            duration_ms: None,
            status: QueueStatus::Queued,
            progress: 0,
            status_detail: Some("Waiting in queue".to_string()),
            detected_language: None,
            is_silent: false,
            segments: Vec::new(),
            txt_path: None,
            srt_path: None,
            vtt_path: None,
            error_code: None,
            error_message: None,
            source: source.to_string(),
            created_at: Utc::now().to_rfc3339(),
            completed_at: None,
        };

        {
            let mut state = self.inner.state.lock().expect("state lock");
            state.items.push(item.clone());
        }
        self.persist();
        self.inner.notify.notify_all();
        self.emit_update();
        item
    }

    fn push_item_and_error(&self, item: QueueItem, error_code: &str, message: &str) {
        {
            let mut state = self.inner.state.lock().expect("state lock");
            state.error_log.insert(
                0,
                ErrorLogEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    timestamp: Utc::now().to_rfc3339(),
                    file_path: item.file_path.clone(),
                    error_code: error_code.to_string(),
                    message: message.to_string(),
                },
            );
            state.items.push(item);
        }
        self.persist();
        self.emit_update();
    }

    fn next_queued_item(&self) -> Option<(String, PathBuf, Option<String>)> {
        let state = self.inner.state.lock().expect("state lock");
        let lang_hint = state.language_hint.clone();
        state
            .items
            .iter()
            .find(|i| i.status == QueueStatus::Queued)
            .map(|i| (i.id.clone(), PathBuf::from(&i.file_path), lang_hint))
    }

    fn update_item<F: FnOnce(&mut QueueItem)>(&self, id: &str, f: F) {
        {
            let mut state = self.inner.state.lock().expect("state lock");
            if let Some(item) = state.items.iter_mut().find(|i| i.id == id) {
                f(item);
            }
        }
        self.persist();
        self.emit_update();
    }

    fn fail_item(&self, id: &str, file_path: &Path, code: &str, message: &str) {
        {
            let mut state = self.inner.state.lock().expect("state lock");
            if let Some(item) = state.items.iter_mut().find(|i| i.id == id) {
                item.status = QueueStatus::Failed;
                item.status_detail = Some(message.to_string());
                item.error_code = Some(code.to_string());
                item.error_message = Some(message.to_string());
            }
            state.error_log.insert(
                0,
                ErrorLogEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    timestamp: Utc::now().to_rfc3339(),
                    file_path: file_path.to_string_lossy().to_string(),
                    error_code: code.to_string(),
                    message: message.to_string(),
                },
            );
            if state.error_log.len() > 250 {
                state.error_log.truncate(250);
            }
        }
        self.persist();
        self.emit_update();
    }

    fn worker_loop(&self) {
        loop {
            let next = self.next_queued_item();
            let Some((id, path, lang_hint)) = next else {
                let guard = self.inner.state.lock().expect("state lock");
                let _ = self
                    .inner
                    .notify
                    .wait_timeout(guard, std::time::Duration::from_millis(500));
                continue;
            };

            if let Ok(mut active) = self.inner.active_worker_id.lock() {
                *active = Some(id.clone());
            }
            self.process_item(&id, &path, lang_hint.as_deref());
            if let Ok(mut active) = self.inner.active_worker_id.lock() {
                *active = None;
            }
            self.emit_update();
        }
    }

    fn process_item(&self, id: &str, path: &Path, lang_hint: Option<&str>) {
        // 1. Decode & VAD segmentation
        self.update_item(id, |item| {
            item.status = QueueStatus::Decoding;
            item.progress = 10;
            item.status_detail = Some("Decoding audio to 16 kHz mono PCM...".to_string());
        });

        let prepared = match audio::prepare_audio_for_whisper(path) {
            Ok(p) => p,
            Err(err) => {
                self.fail_item(id, path, err.code(), &err.message());
                return;
            }
        };

        let duration_ms = prepared.duration_ms;
        let is_silent = prepared.is_silent;

        self.update_item(id, |item| {
            item.duration_ms = Some(duration_ms);
            item.is_silent = is_silent;
            item.progress = 20;
            item.status_detail = Some(if is_silent {
                "Silence detected; skipping speech inference".to_string()
            } else {
                format!(
                    "Prepared {} speech chunk(s) for Whisper",
                    prepared.chunks.len()
                )
            });
        });

        // 2. Transcribe with whisper.cpp (or emit empty segments for pure silence)
        let (detected_language, segments) = if is_silent {
            ("silence".to_string(), Vec::new())
        } else {
            self.update_item(id, |item| {
                item.status = QueueStatus::Transcribing;
                item.progress = 22;
                item.status_detail = Some("Running Whisper speech recognition...".to_string());
            });

            let mgr_ref = self.clone();
            let id_owned = id.to_string();
            let res = whisper::transcribe_chunks(&prepared.chunks, lang_hint, |done, total| {
                let pct = 22 + ((done * 66) / total.max(1)) as u8;
                mgr_ref.update_item(&id_owned, |item| {
                    item.progress = pct.min(89);
                    item.status_detail =
                        Some(format!("Transcribed chunk {} of {}", done, total));
                });
            });

            match res {
                Ok(t) => (t.detected_language, t.segments),
                Err(err) => {
                    prepared.cleanup();
                    self.fail_item(id, path, err.code(), &err.message());
                    return;
                }
            }
        };

        prepared.cleanup();

        // 3. Export TXT, SRT, VTT next to the source file
        self.update_item(id, |item| {
            item.status = QueueStatus::Exporting;
            item.progress = 92;
            item.status_detail = Some("Saving TXT, SRT, and VTT files...".to_string());
        });

        let exports = match export::export_all_formats(path, &segments) {
            Ok(p) => p,
            Err(err) => {
                self.fail_item(id, path, err.code(), &err.message());
                return;
            }
        };

        let txt_str = exports.txt_path.to_string_lossy().to_string();
        let srt_str = exports.srt_path.to_string_lossy().to_string();
        let vtt_str = exports.vtt_path.to_string_lossy().to_string();
        let completed_at = Utc::now().to_rfc3339();
        let canon = canonical_or_raw(path);

        {
            let mut state = self.inner.state.lock().expect("state lock");
            let mut item_sha = String::new();
            if let Some(item) = state.items.iter_mut().find(|i| i.id == id) {
                item.status = QueueStatus::Completed;
                item.progress = 100;
                item.status_detail = Some("Completed (TXT, SRT, VTT saved)".to_string());
                item.detected_language = Some(detected_language.clone());
                item.segments = segments.clone();
                item.txt_path = Some(txt_str.clone());
                item.srt_path = Some(srt_str.clone());
                item.vtt_path = Some(vtt_str.clone());
                item.completed_at = Some(completed_at.clone());
                item_sha = item.sha256.clone().unwrap_or_default();
            }
            if !item_sha.is_empty() {
                state.processed_hashes.insert(
                    item_sha.clone(),
                    ProcessedRecord {
                        sha256: item_sha.clone(),
                        file_path: canon.clone(),
                        txt_path: txt_str,
                        srt_path: srt_str,
                        vtt_path: vtt_str,
                        detected_language,
                        processed_at: completed_at,
                    },
                );
                state.processed_paths.insert(canon, item_sha);
            }
        }
        self.persist();
        self.emit_update();
    }
}
