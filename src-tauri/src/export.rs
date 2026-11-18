use crate::models::TranscriptSegment;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ExportPaths {
    pub txt_path: PathBuf,
    pub srt_path: PathBuf,
    pub vtt_path: PathBuf,
}

#[derive(Debug)]
pub enum ExportError {
    PermissionDenied { path: String, detail: String },
    IoError { path: String, detail: String },
}

impl ExportError {
    pub fn code(&self) -> &'static str {
        match self {
            ExportError::PermissionDenied { .. } => "PERMISSION_DENIED",
            ExportError::IoError { .. } => "EXPORT_IO_ERROR",
        }
    }

    pub fn message(&self) -> String {
        match self {
            ExportError::PermissionDenied { path, detail } => {
                format!("No write permission for output in '{}': {}", path, detail)
            }
            ExportError::IoError { path, detail } => {
                format!("Failed writing output '{}': {}", path, detail)
            }
        }
    }
}

pub fn format_timestamp_srt(ms: u64) -> String {
    let total_secs = ms / 1000;
    let millis = ms % 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{:02}:{:02}:{:02},{:03}", hours, minutes, seconds, millis)
}

pub fn format_timestamp_vtt(ms: u64) -> String {
    let total_secs = ms / 1000;
    let millis = ms % 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, seconds, millis)
}

pub fn render_txt(segments: &[TranscriptSegment]) -> String {
    if segments.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = segments
        .iter()
        .map(|s| s.text.trim())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

pub fn render_srt(segments: &[TranscriptSegment]) -> String {
    let non_empty: Vec<&TranscriptSegment> = segments
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .collect();
    if non_empty.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (idx, seg) in non_empty.iter().enumerate() {
        let end_ms = seg.end_ms.max(seg.start_ms + 200);
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            idx + 1,
            format_timestamp_srt(seg.start_ms),
            format_timestamp_srt(end_ms),
            seg.text.trim()
        ));
    }
    out
}

pub fn render_vtt(segments: &[TranscriptSegment]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for seg in segments.iter().filter(|s| !s.text.trim().is_empty()) {
        let end_ms = seg.end_ms.max(seg.start_ms + 200);
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            format_timestamp_vtt(seg.start_ms),
            format_timestamp_vtt(end_ms),
            seg.text.trim()
        ));
    }
    out
}

fn write_atomic(target: &Path, content: &str) -> Result<(), ExportError> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let tmp_path = parent.join(format!(".{}.{}.tmp", file_name, uuid::Uuid::new_v4()));

    let mut file = match fs::File::create(&tmp_path) {
        Ok(f) => f,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                return Err(ExportError::PermissionDenied {
                    path: parent.display().to_string(),
                    detail: e.to_string(),
                });
            }
            return Err(ExportError::IoError {
                path: target.display().to_string(),
                detail: e.to_string(),
            });
        }
    };

    if let Err(e) = file.write_all(content.as_bytes()).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&tmp_path);
        return Err(ExportError::IoError {
            path: target.display().to_string(),
            detail: e.to_string(),
        });
    }
    drop(file);

    if let Err(e) = fs::rename(&tmp_path, target) {
        let _ = fs::remove_file(&tmp_path);
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            return Err(ExportError::PermissionDenied {
                path: target.display().to_string(),
                detail: e.to_string(),
            });
        }
        return Err(ExportError::IoError {
            path: target.display().to_string(),
            detail: e.to_string(),
        });
    }

    Ok(())
}

pub fn expected_export_paths(source_path: &Path) -> ExportPaths {
    let stem = source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("transcript");
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    ExportPaths {
        txt_path: parent.join(format!("{}.txt", stem)),
        srt_path: parent.join(format!("{}.srt", stem)),
        vtt_path: parent.join(format!("{}.vtt", stem)),
    }
}

pub fn export_all_formats(
    source_path: &Path,
    segments: &[TranscriptSegment],
) -> Result<ExportPaths, ExportError> {
    let paths = expected_export_paths(source_path);
    let txt = render_txt(segments);
    let srt = render_srt(segments);
    let vtt = render_vtt(segments);

    write_atomic(&paths.txt_path, &txt)?;
    write_atomic(&paths.srt_path, &srt)?;
    write_atomic(&paths.vtt_path, &vtt)?;

    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_srt_and_vtt_correctly() {
        let segments = vec![
            TranscriptSegment {
                start_ms: 0,
                end_ms: 2500,
                text: "Hello world.".into(),
                language: Some("en".into()),
            },
            TranscriptSegment {
                start_ms: 3_602_120,
                end_ms: 3_605_500,
                text: "One hour mark.".into(),
                language: Some("en".into()),
            },
        ];
        let srt = render_srt(&segments);
        assert!(srt.contains("1\n00:00:00,000 --> 00:00:02,500\nHello world."));
        assert!(srt.contains("2\n01:00:02,120 --> 01:00:05,500\nOne hour mark."));

        let vtt = render_vtt(&segments);
        assert!(vtt.starts_with("WEBVTT\n\n"));
        assert!(vtt.contains("01:00:02.120 --> 01:00:05.500\nOne hour mark."));

        let txt = render_txt(&segments);
        assert_eq!(txt, "Hello world.\nOne hour mark.\n");
    }
}
