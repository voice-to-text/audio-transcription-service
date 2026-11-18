use crate::audio::{hidden_command, AudioChunk};
use crate::models::{EngineInfo, TranscriptSegment};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum WhisperError {
    BinaryMissing(String),
    ModelMissing(String),
    ExecutionFailed(String),
    ParseFailed(String),
}

impl WhisperError {
    pub fn code(&self) -> &'static str {
        match self {
            WhisperError::BinaryMissing(_) => "WHISPER_BINARY_MISSING",
            WhisperError::ModelMissing(_) => "WHISPER_MODEL_MISSING",
            WhisperError::ExecutionFailed(_) => "WHISPER_EXECUTION_FAILED",
            WhisperError::ParseFailed(_) => "WHISPER_OUTPUT_PARSE_FAILED",
        }
    }

    pub fn message(&self) -> String {
        match self {
            WhisperError::BinaryMissing(msg) => msg.clone(),
            WhisperError::ModelMissing(msg) => msg.clone(),
            WhisperError::ExecutionFailed(msg) => msg.clone(),
            WhisperError::ParseFailed(msg) => msg.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct WhisperJsonOutput {
    #[serde(default)]
    result: Option<WhisperJsonResult>,
    #[serde(default)]
    transcription: Vec<WhisperJsonSegment>,
}

#[derive(Debug, Deserialize)]
struct WhisperJsonResult {
    language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WhisperJsonSegment {
    offsets: WhisperJsonOffsets,
    text: String,
}

#[derive(Debug, Deserialize)]
struct WhisperJsonOffsets {
    from: i64,
    to: i64,
}

#[derive(Debug)]
pub struct TranscriptionResult {
    pub detected_language: String,
    pub segments: Vec<TranscriptSegment>,
}

fn app_data_models_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join("Library")
                .join("Application Support")
                .join("Audio Transcription Studio")
                .join("models")
        })
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|a| {
            PathBuf::from(a)
                .join("Audio Transcription Studio")
                .join("models")
        })
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join(".local")
                .join("share")
                .join("audio-transcription-studio")
                .join("models")
        })
    }
}

pub fn find_whisper_bin() -> Option<String> {
    if let Ok(custom) = std::env::var("WHISPER_CPP_BIN") {
        let p = Path::new(&custom);
        if p.exists() {
            return Some(custom);
        }
    }

    let bin_names = if cfg!(target_os = "windows") {
        vec!["whisper-cli.exe", "whisper.exe", "main.exe"]
    } else {
        vec!["whisper-cli", "whisper-cpp", "main"]
    };

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let search_dirs = [
                dir.to_path_buf(),
                dir.join("resources"),
                dir.join("../Resources"),
                dir.join("../Resources/resources"),
            ];
            for sd in search_dirs {
                for name in &bin_names {
                    let cand = sd.join(name);
                    if cand.exists() {
                        return Some(cand.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    let fixed_paths = [
        "/opt/homebrew/bin/whisper-cli",
        "/usr/local/bin/whisper-cli",
        "/usr/bin/whisper-cli",
    ];
    for fp in fixed_paths {
        if Path::new(fp).exists() {
            return Some(fp.to_string());
        }
    }

    for name in bin_names {
        if let Ok(out) = hidden_command(name).arg("--help").output() {
            if out.status.success() || !out.stdout.is_empty() || !out.stderr.is_empty() {
                return Some(name.to_string());
            }
        }
    }

    None
}

pub fn find_whisper_model() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("WHISPER_MODEL_PATH") {
        let p = PathBuf::from(custom);
        if is_valid_model_file(&p) {
            return Some(p);
        }
    }

    let preferred_names = [
        "ggml-base.bin",
        "ggml-small.bin",
        "ggml-medium.bin",
        "ggml-tiny.bin",
        "ggml-large-v3-turbo.bin",
        "ggml-large-v3.bin",
    ];

    let mut search_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        search_dirs.push(cwd.join("models"));
        if let Some(parent) = cwd.parent() {
            search_dirs.push(parent.join("models"));
        }
        if let Some(grand) = cwd.parent().and_then(|p| p.parent()) {
            search_dirs.push(grand.join("models"));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            search_dirs.push(dir.join("models"));
            search_dirs.push(dir.join("resources"));
            search_dirs.push(dir.join("resources/models"));
            search_dirs.push(dir.join("../Resources/resources"));
            search_dirs.push(dir.join("../Resources/models"));
            // Walk up from target/release or target/debug in dev runs
            let mut cur = Some(dir);
            for _ in 0..4 {
                if let Some(p) = cur {
                    search_dirs.push(p.join("models"));
                    cur = p.parent();
                }
            }
        }
    }
    if let Some(app_dir) = app_data_models_dir() {
        search_dirs.push(app_dir);
    }

    for dir in &search_dirs {
        for name in preferred_names {
            let cand = dir.join(name);
            if is_valid_model_file(&cand) {
                return Some(cand);
            }
        }
    }

    // Fallback: any ggml-*.bin file in those directories
    for dir in &search_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if let Some(fname) = p.file_name().and_then(|s| s.to_str()) {
                    if fname.starts_with("ggml-")
                        && fname.ends_with(".bin")
                        && is_valid_model_file(&p)
                    {
                        return Some(p);
                    }
                }
            }
        }
    }

    None
}

fn is_valid_model_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 1_000_000)
        .unwrap_or(false)
}

pub fn engine_info(offline_mode: bool) -> EngineInfo {
    let whisper_bin = find_whisper_bin();
    let model_path = find_whisper_model();
    let model_name = model_path
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    let ffmpeg_bin = crate::audio::find_ffmpeg();
    let ready = whisper_bin.is_some() && model_path.is_some();

    EngineInfo {
        whisper_bin,
        model_path: model_path.map(|p| p.to_string_lossy().to_string()),
        model_name,
        ffmpeg_bin,
        ready,
        offline_mode,
    }
}

fn is_non_speech_marker(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    if (lower.starts_with('[') && lower.ends_with(']'))
        || (lower.starts_with('(') && lower.ends_with(')'))
        || (lower.starts_with('*') && lower.ends_with('*'))
    {
        return true;
    }
    matches!(
        lower.as_str(),
        "♪" | "music" | "silence" | "[blank_audio]" | "(blank audio)"
    )
}

pub fn classify_language(chunk_langs: &[String], segments: &[TranscriptSegment]) -> String {
    let mut cyrillic_count: usize = 0;
    let mut latin_count: usize = 0;

    for seg in segments {
        for ch in seg.text.chars() {
            if ('\u{0400}'..='\u{04FF}').contains(&ch) {
                cyrillic_count += 1;
            } else if ch.is_ascii_alphabetic() {
                latin_count += 1;
            }
        }
    }

    let total_alpha = cyrillic_count + latin_count;
    let unique_langs: HashSet<String> = chunk_langs
        .iter()
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty() && l != "auto")
        .collect();

    if (cyrillic_count >= 5 && latin_count >= 5)
        && ((cyrillic_count * 100 / total_alpha.max(1) >= 12
            && latin_count * 100 / total_alpha.max(1) >= 12)
            || (unique_langs.contains("ru") && unique_langs.contains("en")))
    {
        return "mixed (ru+en)".to_string();
    }

    if unique_langs.contains("ru") && unique_langs.contains("en") && total_alpha > 0 {
        return "mixed (ru+en)".to_string();
    }

    if cyrillic_count > latin_count && cyrillic_count >= 3 {
        return "ru".to_string();
    }
    if latin_count > cyrillic_count && latin_count >= 3 {
        // Respect Whisper if it detected a Latin-script language like fr/de/es, else en
        if let Some(first) = chunk_langs.first() {
            let l = first.trim().to_ascii_lowercase();
            if !l.is_empty() && l != "auto" && l != "ru" {
                return l;
            }
        }
        return "en".to_string();
    }

    chunk_langs
        .first()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn transcribe_chunks<F>(
    chunks: &[AudioChunk],
    language_hint: Option<&str>,
    mut on_chunk_done: F,
) -> Result<TranscriptionResult, WhisperError>
where
    F: FnMut(usize, usize),
{
    let bin = find_whisper_bin().ok_or_else(|| {
        WhisperError::BinaryMissing(
            "whisper-cli executable not found. Install whisper.cpp or bundle whisper-cli."
                .to_string(),
        )
    })?;
    let model = find_whisper_model().ok_or_else(|| {
        WhisperError::ModelMissing(
            "Whisper GGML model not found (expected ggml-base.bin or ggml-tiny.bin in models/)."
                .to_string(),
        )
    })?;

    let threads = std::thread::available_parallelism()
        .map(|n| n.get().min(8).max(2))
        .unwrap_or(4);

    let lang_arg = language_hint
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("auto");

    let mut all_segments: Vec<TranscriptSegment> = Vec::new();
    let mut chunk_langs: Vec<String> = Vec::new();
    let total = chunks.len().max(1);

    for (idx, chunk) in chunks.iter().enumerate() {
        let out_prefix = chunk.wav_path.with_extension("out");
        let json_path = PathBuf::from(format!("{}.json", out_prefix.to_string_lossy()));

        let output = hidden_command(&bin)
            .args([
                "-m",
                &model.to_string_lossy(),
                "-f",
                &chunk.wav_path.to_string_lossy(),
                "-l",
                lang_arg,
                "-t",
                &threads.to_string(),
                "-oj",
                "-of",
                &out_prefix.to_string_lossy(),
                "-np",
                "-sns",
            ])
            .output()
            .map_err(|e| {
                WhisperError::ExecutionFailed(format!("Failed running whisper-cli: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(WhisperError::ExecutionFailed(format!(
                "whisper-cli exited with {}: {}",
                output.status,
                stderr.trim()
            )));
        }

        let json_raw = std::fs::read_to_string(&json_path).map_err(|e| {
            WhisperError::ParseFailed(format!(
                "Missing whisper JSON output '{}': {}",
                json_path.display(),
                e
            ))
        })?;
        let _ = std::fs::remove_file(&json_path);

        let parsed: WhisperJsonOutput = serde_json::from_str(&json_raw).map_err(|e| {
            WhisperError::ParseFailed(format!("Invalid whisper JSON output: {}", e))
        })?;

        let chunk_lang = parsed
            .result
            .and_then(|r| r.language)
            .unwrap_or_else(|| "auto".to_string());
        chunk_langs.push(chunk_lang.clone());

        for seg in parsed.transcription {
            let clean = seg.text.trim().to_string();
            if is_non_speech_marker(&clean) {
                continue;
            }
            let rel_from = seg.offsets.from.max(0) as u64;
            let rel_to = seg.offsets.to.max(seg.offsets.from).max(0) as u64;
            let abs_start = chunk.offset_ms + rel_from;
            let abs_end = (chunk.offset_ms + rel_to).max(abs_start + 150);

            all_segments.push(TranscriptSegment {
                start_ms: abs_start,
                end_ms: abs_end,
                text: clean,
                language: Some(chunk_lang.clone()),
            });
        }

        on_chunk_done(idx + 1, total);
    }

    // Ensure monotonically non-decreasing start timestamps
    all_segments.sort_by_key(|s| (s.start_ms, s.end_ms));

    let detected_language = classify_language(&chunk_langs, &all_segments);
    Ok(TranscriptionResult {
        detected_language,
        segments: all_segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_english_russian_and_mixed() {
        let en = vec![TranscriptSegment {
            start_ms: 0,
            end_ms: 1000,
            text: "Hello from Audio Transcription Studio.".into(),
            language: Some("en".into()),
        }];
        assert_eq!(classify_language(&["en".into()], &en), "en");

        let ru = vec![TranscriptSegment {
            start_ms: 0,
            end_ms: 1000,
            text: "Привет, это тестовая запись речи.".into(),
            language: Some("ru".into()),
        }];
        assert_eq!(classify_language(&["ru".into()], &ru), "ru");

        let mixed = vec![
            TranscriptSegment {
                start_ms: 0,
                end_ms: 2000,
                text: "Сегодня мы обсуждаем новый релиз.".into(),
                language: Some("ru".into()),
            },
            TranscriptSegment {
                start_ms: 2100,
                end_ms: 4000,
                text: "Everything runs completely offline on desktop.".into(),
                language: Some("en".into()),
            },
        ];
        assert_eq!(
            classify_language(&["ru".into(), "en".into()], &mixed),
            "mixed (ru+en)"
        );
    }
}
