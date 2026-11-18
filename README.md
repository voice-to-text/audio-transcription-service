# Audio Transcription Studio: Offline Speech-to-Text for macOS and Windows

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](./LICENSE)
[![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20Windows-blue.svg)](#local-development-and-build)
[![Website](https://img.shields.io/badge/Website-GitHub%20Pages-0f766e)](https://voice-to-text.github.io/audio-transcription-service/)

Audio Transcription Studio is a free desktop audio and video transcription tool built with Tauri 2, Rust, React, and whisper.cpp. It turns MP3, WAV, M4A, and MP4 files into TXT, SRT, and VTT files right on your machine with no accounts, API keys, or internet access.

Documentation and guides: [https://voice-to-text.github.io/audio-transcription-service/](https://voice-to-text.github.io/audio-transcription-service/) | [All 22 Topic Guides](https://voice-to-text.github.io/audio-transcription-service/guides.html) | [Semantic Core and Keyword Clusters](./SEO_CORE.md)

## Comparison

| Feature | Audio Transcription Studio | MacWhisper | Cloud Transcription APIs |
| :--- | :--- | :--- | :--- |
| Price | Free (MIT) | Freemium / Pro license | Per-minute billing |
| Platforms | macOS and Windows | macOS only | Browser / API |
| Offline processing | Local whisper.cpp | Local (Pro for batch) | Cloud upload required |
| Watch folder automation | Built-in with copy check | Pro tier | Separate scripts needed |
| Output formats | TXT, SRT, VTT next to file | Manual or Pro export | JSON / VTT download |
| Duplicate file guard | SHA-256 hash tracking | No | No |

## Features

- Drag and drop MP3, WAV, M4A, and MP4 files directly into the app window or pick them from the system file dialog.
- Watch Folder monitors a chosen directory, waits until incoming files finish copying, skips duplicates, and writes TXT, SRT, and VTT sidecar files next to each recording.
- Automatic language detection identifies English, Russian, and mixed Russian/English speech across segments.
- Built-in audio and video player streams local files over HTTP byte ranges so clicking any timestamp jumps playback straight to that moment.
- Persistent queue tracks multi-file progress, isolates corrupted files or folder permission errors in the Error Log, and resumes unfinished jobs if the app closes and restarts.
- Energy-based silence and voice activity segmentation avoids Whisper hallucinations on silent audio and handles 10-second clips, 5-minute interviews, and 1-hour recordings.

## Quick start and installation

Install `whisper.cpp` on macOS (or place `whisper-cli.exe` on Windows alongside the app) and place a multilingual GGML model such as `ggml-base.bin` in `models/` or your app data models folder:

```bash
brew install whisper-cpp
mkdir -p models
curl -L -o models/ggml-base.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin
```

Install frontend dependencies and launch the desktop app:

```bash
npm install
npm run tauri dev
```

Build release installers (`.app`, `.dmg`, `.msi`, `.exe`):

```bash
npm run tauri build
```

## End-to-end test suite

With `whisper-cli` and `models/ggml-base.bin` installed, build the release binary and run the 10-part end-to-end suite covering all formats, languages (English, Russian, mixed), durations (10s, 5m, 1h), noise, multi-speaker dialogue, silence, 10-file watch folder copy stability, duplicate skipping, error handling, restart recovery, and offline byte-range playback:

```bash
node tests/run_e2e_tests.mjs
```

## Architecture

- [`src-tauri/src/audio.rs`](./src-tauri/src/audio.rs): Pure-Rust `symphonia` decoder (with `ffmpeg` fallback), 16 kHz mono resampler, silence gate, and VAD chunker.
- [`src-tauri/src/whisper.rs`](./src-tauri/src/whisper.rs): `whisper-cli` execution, segment timestamp alignment, hallucination filtering, and script-aware language classification.
- [`src-tauri/src/export.rs`](./src-tauri/src/export.rs): Atomic TXT, SRT, and WEBVTT writers with write-permission checking.
- [`src-tauri/src/queue.rs`](./src-tauri/src/queue.rs): Persistent `queue_state.json`, SHA-256 deduplication, error log, and restart recovery worker.
- [`src-tauri/src/watcher.rs`](./src-tauri/src/watcher.rs): Cross-platform `notify` directory watcher with file-size copy stability checks.
- [`src-tauri/src/api.rs`](./src-tauri/src/api.rs): Tauri IPC commands and local HTTP server (`127.0.0.1:4895`) for byte-range media streaming.
- [`src/App.tsx`](./src/App.tsx): React desktop interface with drag-and-drop, queue progress, clickable timestamp player, and error log.

## Frequently asked questions

### Does Audio Transcription Studio work without internet access?
Yes. Decoding, voice activity chunking, Whisper speech recognition, and subtitle export all run locally on your CPU or GPU without network requests or API keys.

### Which file formats are supported?
The app accepts MP3, WAV, M4A, and MP4 files. Unsupported extensions and damaged files are flagged in the Error Log without blocking the rest of the queue.

### How does the Watch Folder prevent half-copied files from failing?
When a new file appears in the watched folder, the watcher checks its byte size across consecutive intervals and waits until the size stops changing and the file opens cleanly before starting transcription.

## License

[MIT License](./LICENSE)
