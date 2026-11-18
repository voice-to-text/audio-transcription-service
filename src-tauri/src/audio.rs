use hound::{SampleFormat, WavSpec, WavWriter};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub const TARGET_SAMPLE_RATE: u32 = 16_000;
const SUPPORTED_EXTENSIONS: [&str; 4] = ["mp3", "wav", "m4a", "mp4"];

#[derive(Debug)]
pub enum AudioError {
    UnsupportedFormat(String),
    CorruptedFile(String),
    PermissionDenied(String),
    IoError(String),
}

impl AudioError {
    pub fn code(&self) -> &'static str {
        match self {
            AudioError::UnsupportedFormat(_) => "UNSUPPORTED_FORMAT",
            AudioError::CorruptedFile(_) => "CORRUPTED_FILE",
            AudioError::PermissionDenied(_) => "PERMISSION_DENIED",
            AudioError::IoError(_) => "AUDIO_IO_ERROR",
        }
    }

    pub fn message(&self) -> String {
        match self {
            AudioError::UnsupportedFormat(ext) => {
                format!(
                    "Unsupported file format '{}'. Supported formats: MP3, WAV, M4A, MP4.",
                    ext
                )
            }
            AudioError::CorruptedFile(reason) => {
                format!("Corrupted or unreadable media file: {}", reason)
            }
            AudioError::PermissionDenied(reason) => {
                format!("Permission denied accessing media file: {}", reason)
            }
            AudioError::IoError(reason) => format!("Media I/O error: {}", reason),
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AudioChunk {
    pub index: usize,
    pub offset_ms: u64,
    pub duration_ms: u64,
    pub wav_path: PathBuf,
}

#[derive(Debug)]
pub struct PreparedAudio {
    pub duration_ms: u64,
    pub is_silent: bool,
    pub chunks: Vec<AudioChunk>,
    pub temp_dir: PathBuf,
}

impl PreparedAudio {
    pub fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}

pub fn is_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn extension_lowercase(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn find_ffmpeg() -> Option<String> {
    if let Ok(custom) = std::env::var("FFMPEG_BIN") {
        if Path::new(&custom).exists() {
            return Some(custom);
        }
    }
    let candidates = [
        "ffmpeg",
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ];
    for c in candidates {
        if c == "ffmpeg" {
            if let Ok(out) = hidden_command(c).arg("-version").output() {
                if out.status.success() {
                    return Some(c.to_string());
                }
            }
        } else if Path::new(c).exists() {
            return Some(c.to_string());
        }
    }
    None
}

pub fn hidden_command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Resample mono f32 samples from `src_rate` to `TARGET_SAMPLE_RATE` (16 kHz) using linear interpolation.
fn resample_to_16k(mono: &[f32], src_rate: u32, out: &mut Vec<i16>) {
    if mono.is_empty() || src_rate == 0 {
        return;
    }
    if src_rate == TARGET_SAMPLE_RATE {
        out.extend(
            mono.iter()
                .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16),
        );
        return;
    }
    let ratio = src_rate as f64 / TARGET_SAMPLE_RATE as f64;
    let out_len = ((mono.len() as f64) / ratio).floor() as usize;
    out.reserve(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let s0 = mono[idx.min(mono.len() - 1)];
        let s1 = mono[(idx + 1).min(mono.len() - 1)];
        let sample = s0 + (s1 - s0) * frac;
        out.push((sample.clamp(-1.0, 1.0) * 32767.0) as i16);
    }
}

fn decode_with_symphonia(path: &Path) -> Result<Vec<i16>, AudioError> {
    let file = File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            AudioError::PermissionDenied(e.to_string())
        } else {
            AudioError::IoError(e.to_string())
        }
    })?;

    let meta = file
        .metadata()
        .map_err(|e| AudioError::IoError(e.to_string()))?;
    if meta.len() < 16 {
        return Err(AudioError::CorruptedFile(
            "File is empty or too small to contain valid audio".to_string(),
        ));
    }

    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| AudioError::CorruptedFile(format!("Container probe failed: {}", e)))?;

    let mut format = probed.format;

    // Pick first decodable audio track (skips video tracks in MP4).
    let mut chosen = None;
    for track in format.tracks() {
        if track.codec_params.codec == CODEC_TYPE_NULL {
            continue;
        }
        if let Ok(decoder) =
            symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())
        {
            chosen = Some((track.id, decoder));
            break;
        }
    }

    let (track_id, mut decoder) = chosen.ok_or_else(|| {
        AudioError::CorruptedFile("No supported audio stream found in media file".to_string())
    })?;

    let mut samples_16k: Vec<i16> = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut decode_errors = 0usize;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(err) => {
                if samples_16k.is_empty() {
                    return Err(AudioError::CorruptedFile(format!(
                        "Stream read error: {}",
                        err
                    )));
                }
                break;
            }
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let channels = spec.channels.count().max(1);
                let rate = spec.rate;

                let buf = match sample_buf {
                    Some(ref mut b) if b.capacity() >= decoded.capacity() => b,
                    _ => {
                        sample_buf = Some(SampleBuffer::<f32>::new(
                            decoded.capacity() as u64,
                            spec,
                        ));
                        sample_buf.as_mut().expect("sample_buf just created")
                    }
                };
                buf.copy_interleaved_ref(decoded);
                let interleaved = buf.samples();

                let mut mono = Vec::with_capacity(interleaved.len() / channels);
                for frame in interleaved.chunks_exact(channels) {
                    let sum: f32 = frame.iter().copied().sum();
                    mono.push(sum / channels as f32);
                }
                resample_to_16k(&mono, rate, &mut samples_16k);
            }
            Err(SymphoniaError::DecodeError(msg)) => {
                decode_errors += 1;
                if decode_errors > 32 && samples_16k.is_empty() {
                    return Err(AudioError::CorruptedFile(format!(
                        "Too many decode errors: {}",
                        msg
                    )));
                }
            }
            Err(SymphoniaError::IoError(_)) => break,
            Err(err) => {
                if samples_16k.is_empty() {
                    return Err(AudioError::CorruptedFile(format!("Decode error: {}", err)));
                }
                break;
            }
        }
    }

    if samples_16k.is_empty() {
        return Err(AudioError::CorruptedFile(
            "Decoded audio contains 0 samples".to_string(),
        ));
    }

    Ok(samples_16k)
}

fn decode_with_ffmpeg(path: &Path, temp_dir: &Path) -> Result<Vec<i16>, AudioError> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| {
        AudioError::CorruptedFile("Native decoder failed and ffmpeg is not available".to_string())
    })?;
    let out_wav = temp_dir.join("ffmpeg_decoded.wav");
    let output = hidden_command(&ffmpeg)
        .args([
            "-y",
            "-v",
            "error",
            "-i",
            &path.to_string_lossy(),
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            &out_wav.to_string_lossy(),
        ])
        .output()
        .map_err(|e| AudioError::IoError(format!("Failed spawning ffmpeg: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AudioError::CorruptedFile(format!(
            "Invalid media stream: {}",
            stderr.trim()
        )));
    }

    let reader = hound::WavReader::open(&out_wav)
        .map_err(|e| AudioError::CorruptedFile(format!("Decoded WAV unreadable: {}", e)))?;
    let samples: Vec<i16> = reader
        .into_samples::<i16>()
        .filter_map(Result::ok)
        .collect();
    let _ = std::fs::remove_file(&out_wav);

    if samples.is_empty() {
        return Err(AudioError::CorruptedFile(
            "Audio stream contains 0 samples".to_string(),
        ));
    }
    Ok(samples)
}

fn write_wav_16k(path: &Path, samples: &[i16]) -> Result<(), AudioError> {
    let spec = WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec)
        .map_err(|e| AudioError::IoError(format!("Failed creating WAV chunk: {}", e)))?;
    for &s in samples {
        writer
            .write_sample(s)
            .map_err(|e| AudioError::IoError(format!("Failed writing WAV sample: {}", e)))?;
    }
    writer
        .finalize()
        .map_err(|e| AudioError::IoError(format!("Failed finalizing WAV chunk: {}", e)))?;
    Ok(())
}

/// Prepare audio for Whisper: validates format, decodes to 16 kHz mono PCM,
/// detects silence, and segments long audio into VAD-aligned WAV chunks.
pub fn prepare_audio_for_whisper(path: &Path) -> Result<PreparedAudio, AudioError> {
    if !path.exists() {
        return Err(AudioError::IoError(format!(
            "File does not exist: {}",
            path.display()
        )));
    }

    let ext = extension_lowercase(path);
    if !is_supported_extension(path) {
        return Err(AudioError::UnsupportedFormat(ext));
    }

    // Check file readability early so permission errors are reported accurately.
    if let Err(e) = File::open(path) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            return Err(AudioError::PermissionDenied(e.to_string()));
        }
        return Err(AudioError::IoError(e.to_string()));
    }

    let temp_dir = std::env::temp_dir().join(format!("ats_job_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| AudioError::IoError(format!("Failed creating temp dir: {}", e)))?;

    let samples = match decode_with_symphonia(path) {
        Ok(s) => s,
        Err(sym_err) => {
            if matches!(sym_err, AudioError::PermissionDenied(_)) {
                let _ = std::fs::remove_dir_all(&temp_dir);
                return Err(sym_err);
            }
            match decode_with_ffmpeg(path, &temp_dir) {
                Ok(s) => s,
                Err(_) => {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return Err(sym_err);
                }
            }
        }
    };

    let total_samples = samples.len();
    let duration_ms = ((total_samples as u64) * 1000) / (TARGET_SAMPLE_RATE as u64);

    // Analyze 100 ms windows (1600 samples) for silence and VAD segmentation.
    const WIN_SAMPLES: usize = 1600;
    let mut win_rms: Vec<f32> = Vec::with_capacity((total_samples + WIN_SAMPLES - 1) / WIN_SAMPLES);
    let mut max_peak: i16 = 0;

    for chunk in samples.chunks(WIN_SAMPLES) {
        let mut sum_sq: f64 = 0.0;
        for &s in chunk {
            let abs_s = s.saturating_abs();
            if abs_s > max_peak {
                max_peak = abs_s;
            }
            let norm = (s as f64) / 32768.0;
            sum_sq += norm * norm;
        }
        let rms = (sum_sq / chunk.len() as f64).sqrt() as f32;
        win_rms.push(rms);
    }

    let max_rms = win_rms.iter().copied().fold(0.0f32, f32::max);
    let is_silent = max_rms < 0.0012 && max_peak < 120;

    if is_silent {
        return Ok(PreparedAudio {
            duration_ms,
            is_silent: true,
            chunks: Vec::new(),
            temp_dir,
        });
    }

    // Segment audio on genuine pauses (> ~0.55s) so multi-speaker turns,
    // language switches, and long recordings (5m / 1h) keep clean timestamps.
    let mut ranges = segment_audio_by_vad(&win_rms, total_samples, WIN_SAMPLES);
    if ranges.is_empty() {
        ranges.push((0, total_samples));
    }

    let mut chunks = Vec::with_capacity(ranges.len());
    for (idx, (start_sample, end_sample)) in ranges.into_iter().enumerate() {
        let end_clamped = end_sample.min(total_samples);
        if end_clamped <= start_sample {
            continue;
        }
        let wav_path = temp_dir.join(format!("chunk_{:04}.wav", idx));
        write_wav_16k(&wav_path, &samples[start_sample..end_clamped])?;
        let offset_ms = ((start_sample as u64) * 1000) / (TARGET_SAMPLE_RATE as u64);
        let chunk_dur_ms =
            (((end_clamped - start_sample) as u64) * 1000) / (TARGET_SAMPLE_RATE as u64);
        chunks.push(AudioChunk {
            index: idx,
            offset_ms,
            duration_ms: chunk_dur_ms,
            wav_path,
        });
    }

    Ok(PreparedAudio {
        duration_ms,
        is_silent: chunks.is_empty(),
        chunks,
        temp_dir,
    })
}

/// Segment audio into speech regions using energy VAD, skipping pauses > ~0.55s
/// and capping continuous active speech chunks at 26 seconds.
fn segment_audio_by_vad(
    win_rms: &[f32],
    total_samples: usize,
    win_samples: usize,
) -> Vec<(usize, usize)> {
    const ACTIVE_RMS_THRESHOLD: f32 = 0.0022;
    const HANGOVER_WINDOWS: usize = 3; // 0.3s trailing tolerance
    const PAD_WINDOWS: usize = 2; // 0.2s lead padding
    const MAX_CHUNK_WINDOWS: usize = 260; // 26.0s max per chunk
    const MIN_SPLIT_WINDOWS: usize = 200; // 20.0s before seeking quietest cut

    let n = win_rms.len();
    let mut is_active = vec![false; n];
    for (i, &r) in win_rms.iter().enumerate() {
        if r >= ACTIVE_RMS_THRESHOLD {
            let start = i.saturating_sub(PAD_WINDOWS);
            let end = (i + HANGOVER_WINDOWS + 1).min(n);
            for flag in &mut is_active[start..end] {
                *flag = true;
            }
        }
    }

    let mut ranges = Vec::new();
    let mut i = 0;
    while i < n {
        if !is_active[i] {
            i += 1;
            continue;
        }
        let seg_start = i;
        while i < n && is_active[i] && (i - seg_start) < MAX_CHUNK_WINDOWS {
            i += 1;
        }
        let mut seg_end = i;

        // If we hit MAX_CHUNK_WINDOWS and speech continues, cut at the lowest-energy window near the end.
        if i < n && is_active[i] && (seg_end - seg_start) >= MAX_CHUNK_WINDOWS {
            let search_from = seg_start + MIN_SPLIT_WINDOWS;
            if search_from < seg_end {
                let mut best_idx = seg_end;
                let mut best_rms = f32::MAX;
                for k in search_from..seg_end {
                    if win_rms[k] < best_rms {
                        best_rms = win_rms[k];
                        best_idx = k;
                    }
                }
                if best_idx > seg_start + 20 {
                    seg_end = best_idx;
                    i = best_idx;
                }
            }
        }

        let start_sample = seg_start * win_samples;
        let end_sample = (seg_end * win_samples).min(total_samples);
        if end_sample > start_sample + (TARGET_SAMPLE_RATE as usize / 4) {
            ranges.push((start_sample, end_sample));
        }
    }

    // Fallback: if nothing passed active threshold despite non-silent peak, split uniformly.
    if ranges.is_empty() {
        let max_samples = MAX_CHUNK_WINDOWS * win_samples;
        let mut pos = 0;
        while pos < total_samples {
            let end = (pos + max_samples).min(total_samples);
            ranges.push((pos, end));
            pos = end;
        }
    }

    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_skips_long_silence_between_speech() {
        // Simulate 1 hour (36,000 windows of 100ms) with speech at 2s, 30m, 59m45s.
        let total_wins = 36_000;
        let mut win_rms = vec![0.0001f32; total_wins];
        for w in 20..55 {
            win_rms[w] = 0.05;
        }
        for w in 18_000..18_040 {
            win_rms[w] = 0.05;
        }
        for w in 35_850..35_885 {
            win_rms[w] = 0.05;
        }
        let ranges = segment_audio_by_vad(&win_rms, total_wins * 1600, 1600);
        assert_eq!(ranges.len(), 3);
        assert!(ranges[0].0 <= 20 * 1600);
        assert!(ranges[1].0 >= 17_900 * 1600 && ranges[1].0 <= 18_000 * 1600);
        assert!(ranges[2].0 >= 35_750 * 1600 && ranges[2].0 <= 35_850 * 1600);
    }
}
