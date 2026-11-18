use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum QueueStatus {
    WaitingForCopy,
    Queued,
    Decoding,
    Transcribing,
    Exporting,
    Completed,
    SkippedDuplicate,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub id: String,
    pub file_path: String,
    pub file_name: String,
    pub format: String,
    pub file_size: u64,
    pub sha256: Option<String>,
    pub duration_ms: Option<u64>,
    pub status: QueueStatus,
    pub progress: u8,
    pub status_detail: Option<String>,
    pub detected_language: Option<String>,
    pub is_silent: bool,
    pub segments: Vec<TranscriptSegment>,
    pub txt_path: Option<String>,
    pub srt_path: Option<String>,
    pub vtt_path: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub source: String,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessedRecord {
    pub sha256: String,
    pub file_path: String,
    pub txt_path: String,
    pub srt_path: String,
    pub vtt_path: String,
    pub detected_language: String,
    pub processed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorLogEntry {
    pub id: String,
    pub timestamp: String,
    pub file_path: String,
    pub error_code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedState {
    pub items: Vec<QueueItem>,
    pub processed_hashes: HashMap<String, ProcessedRecord>,
    pub processed_paths: HashMap<String, String>,
    pub error_log: Vec<ErrorLogEntry>,
    pub watch_folder: Option<String>,
    pub watch_active: bool,
    pub offline_mode: bool,
    pub language_hint: Option<String>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            processed_hashes: HashMap::new(),
            processed_paths: HashMap::new(),
            error_log: Vec::new(),
            watch_folder: None,
            watch_active: false,
            offline_mode: true,
            language_hint: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub whisper_bin: Option<String>,
    pub model_path: Option<String>,
    pub model_name: Option<String>,
    pub ffmpeg_bin: Option<String>,
    pub ready: bool,
    pub offline_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStateSnapshot {
    pub items: Vec<QueueItem>,
    pub error_log: Vec<ErrorLogEntry>,
    pub watch_folder: Option<String>,
    pub watch_active: bool,
    pub offline_mode: bool,
    pub processed_count: usize,
    pub engine: EngineInfo,
    pub active_worker_id: Option<String>,
}
