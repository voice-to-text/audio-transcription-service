use crate::audio::is_supported_extension;
use crate::models::{AppStateSnapshot, QueueItem};
use crate::queue::QueueManager;
use crate::watcher::FolderWatcher;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::State;

#[derive(Clone)]
pub struct AppBackend {
    pub queue: QueueManager,
    pub watcher: FolderWatcher,
    pub port: u16,
}

#[derive(Deserialize)]
struct CommandBody {
    action: String,
    #[serde(default)]
    payload: Value,
}

#[tauri::command]
pub fn get_app_state(state: State<'_, AppBackend>) -> AppStateSnapshot {
    state.queue.snapshot()
}

#[tauri::command]
pub fn get_api_port(state: State<'_, AppBackend>) -> u16 {
    state.port
}

#[tauri::command]
pub fn enqueue_files(
    state: State<'_, AppBackend>,
    paths: Vec<String>,
    force: Option<bool>,
) -> Vec<QueueItem> {
    let force_flag = force.unwrap_or(false);
    paths
        .into_iter()
        .map(|p| state.queue.enqueue_file(Path::new(&p), "manual", force_flag))
        .collect()
}

#[tauri::command]
pub fn set_watch_folder(
    state: State<'_, AppBackend>,
    folder: String,
) -> Result<AppStateSnapshot, String> {
    state.watcher.start_watching(Path::new(&folder))?;
    Ok(state.queue.snapshot())
}

#[tauri::command]
pub fn stop_watch_folder(state: State<'_, AppBackend>) -> AppStateSnapshot {
    state.watcher.stop_watching();
    state.queue.snapshot()
}

#[tauri::command]
pub fn retry_item(state: State<'_, AppBackend>, id: String) -> Option<QueueItem> {
    state.queue.retry_item(&id)
}

#[tauri::command]
pub fn force_retranscribe(state: State<'_, AppBackend>, id: String) -> Option<QueueItem> {
    state.queue.force_retranscribe(&id)
}

#[tauri::command]
pub fn remove_item(state: State<'_, AppBackend>, id: String) -> AppStateSnapshot {
    state.queue.remove_item(&id);
    state.queue.snapshot()
}

#[tauri::command]
pub fn clear_completed(state: State<'_, AppBackend>) -> AppStateSnapshot {
    state.queue.clear_completed();
    state.queue.snapshot()
}

#[tauri::command]
pub fn clear_errors(state: State<'_, AppBackend>) -> AppStateSnapshot {
    state.queue.clear_errors();
    state.queue.snapshot()
}

#[tauri::command]
pub fn read_export_text(path: String) -> Result<String, String> {
    let p = Path::new(&path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(ext.as_str(), "txt" | "srt" | "vtt") {
        return Err("Only .txt, .srt, and .vtt files can be read here".to_string());
    }
    std::fs::read_to_string(p).map_err(|e| e.to_string())
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn extract_query_param(target: &str, key: &str) -> Option<String> {
    let (_, query) = target.split_once('?')?;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == key {
            return Some(percent_decode(v));
        }
    }
    None
}

fn content_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        Some("mp4") => "video/mp4",
        _ => "application/octet-stream",
    }
}

fn write_http_json(stream: &mut TcpStream, status: &str, body: &Value) {
    let body_str = body.to_string();
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json; charset=utf-8\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Range\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        body_str.len(),
        body_str
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

fn serve_media_file(stream: &mut TcpStream, file_path: &Path, range_header: Option<&str>) {
    if !file_path.exists() || !is_supported_extension(file_path) {
        write_http_json(stream, "404 Not Found", &json!({ "error": "Media file not found" }));
        return;
    }

    let mut file = match File::open(file_path) {
        Ok(f) => f,
        Err(e) => {
            write_http_json(
                stream,
                "500 Internal Server Error",
                &json!({ "error": e.to_string() }),
            );
            return;
        }
    };

    let total_len = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            write_http_json(
                stream,
                "500 Internal Server Error",
                &json!({ "error": e.to_string() }),
            );
            return;
        }
    };

    let ctype = content_type_for_path(file_path);

    let (status, start, end) = if let Some(rh) = range_header {
        if let Some(spec) = rh.strip_prefix("bytes=") {
            let (s_str, e_str) = spec.split_once('-').unwrap_or((spec, ""));
            let s = s_str.trim().parse::<u64>().unwrap_or(0);
            let e = if e_str.trim().is_empty() {
                total_len.saturating_sub(1)
            } else {
                e_str
                    .trim()
                    .parse::<u64>()
                    .unwrap_or(total_len.saturating_sub(1))
                    .min(total_len.saturating_sub(1))
            };
            if s > e || s >= total_len {
                let resp = format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
                    total_len
                );
                let _ = stream.write_all(resp.as_bytes());
                return;
            }
            ("206 Partial Content", s, e)
        } else {
            ("200 OK", 0, total_len.saturating_sub(1))
        }
    } else {
        ("200 OK", 0, total_len.saturating_sub(1))
    };

    let chunk_len = if total_len == 0 { 0 } else { end - start + 1 };
    let mut headers = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Content-Type, Range\r\nContent-Length: {}\r\nConnection: close\r\n",
        status, ctype, chunk_len
    );
    if status == "206 Partial Content" {
        headers.push_str(&format!("Content-Range: bytes {}-{}/{}\r\n", start, end, total_len));
    }
    headers.push_str("\r\n");

    if stream.write_all(headers.as_bytes()).is_err() {
        return;
    }

    if chunk_len > 0 {
        if file.seek(SeekFrom::Start(start)).is_err() {
            return;
        }
        let mut remaining = chunk_len;
        let mut buf = [0u8; 65_536];
        while remaining > 0 {
            let to_read = remaining.min(buf.len() as u64) as usize;
            match file.read(&mut buf[..to_read]) {
                Ok(0) => break,
                Ok(n) => {
                    if stream.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    remaining -= n as u64;
                }
                Err(_) => break,
            }
        }
    }
    let _ = stream.flush();
}

fn handle_client(mut stream: TcpStream, backend: AppBackend) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(cloned) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(cloned);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }

    let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
    if parts.len() < 2 {
        write_http_json(&mut stream, "400 Bad Request", &json!({ "error": "Invalid HTTP request" }));
        return;
    }
    let method = parts[0];
    let target = parts[1];
    let path_only = target.split('?').next().unwrap_or(target);

    let mut content_length: usize = 0;
    let mut range_header: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(val) = lower.strip_prefix("content-length:") {
            content_length = val.trim().parse().unwrap_or(0);
        } else if let Some(val) = lower.strip_prefix("range:") {
            range_header = Some(val.trim().to_string());
        }
    }

    if method == "OPTIONS" {
        let resp = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Range\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes());
        return;
    }

    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body_bytes).is_err() {
        write_http_json(&mut stream, "400 Bad Request", &json!({ "error": "Incomplete body" }));
        return;
    }
    let body_json: Value = if body_bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body_bytes).unwrap_or_else(|_| json!({}))
    };

    if method == "GET" && path_only == "/health" {
        let snap = backend.queue.snapshot();
        write_http_json(
            &mut stream,
            "200 OK",
            &json!({
                "ok": true,
                "app": "Audio Transcription Studio",
                "version": "0.1.0",
                "port": backend.port,
                "engine": snap.engine
            }),
        );
        return;
    }

    if method == "GET" && path_only == "/state" {
        let snap = backend.queue.snapshot();
        write_http_json(&mut stream, "200 OK", &serde_json::to_value(snap).unwrap_or(json!({})));
        return;
    }

    if method == "GET" && path_only == "/media" {
        if let Some(media_path) = extract_query_param(target, "path") {
            serve_media_file(&mut stream, Path::new(&media_path), range_header.as_deref());
        } else {
            write_http_json(&mut stream, "400 Bad Request", &json!({ "error": "Missing path param" }));
        }
        return;
    }

    if method == "POST" && path_only == "/command" {
        let cmd: CommandBody = match serde_json::from_value(body_json) {
            Ok(c) => c,
            Err(e) => {
                write_http_json(
                    &mut stream,
                    "400 Bad Request",
                    &json!({ "error": format!("Invalid command: {}", e) }),
                );
                return;
            }
        };

        let res = execute_command(&backend, &cmd.action, &cmd.payload);
        match res {
            Ok(v) => write_http_json(&mut stream, "200 OK", &v),
            Err(err) => write_http_json(&mut stream, "400 Bad Request", &json!({ "ok": false, "error": err })),
        }
        return;
    }

    write_http_json(&mut stream, "404 Not Found", &json!({ "error": "Route not found" }));
}

fn execute_command(backend: &AppBackend, action: &str, payload: &Value) -> Result<Value, String> {
    match action {
        "get_state" => Ok(serde_json::to_value(backend.queue.snapshot()).unwrap_or(json!({}))),
        "enqueue_files" => {
            let paths = payload
                .get("paths")
                .and_then(|v| v.as_array())
                .ok_or_else(|| "Missing payload.paths array".to_string())?;
            let force = payload.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let mut items = Vec::new();
            for p in paths {
                if let Some(s) = p.as_str() {
                    items.push(backend.queue.enqueue_file(PathBuf::from(s).as_path(), "manual", force));
                }
            }
            Ok(json!({ "ok": true, "items": items }))
        }
        "set_watch_folder" => {
            let folder = payload
                .get("folder")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing payload.folder string".to_string())?;
            backend.watcher.start_watching(Path::new(folder))?;
            Ok(json!({ "ok": true, "state": backend.queue.snapshot() }))
        }
        "stop_watch_folder" => {
            backend.watcher.stop_watching();
            Ok(json!({ "ok": true, "state": backend.queue.snapshot() }))
        }
        "retry_item" => {
            let id = payload
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing payload.id".to_string())?;
            Ok(json!({ "ok": true, "item": backend.queue.retry_item(id) }))
        }
        "force_retranscribe" => {
            let id = payload
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing payload.id".to_string())?;
            Ok(json!({ "ok": true, "item": backend.queue.force_retranscribe(id) }))
        }
        "clear_completed" => {
            backend.queue.clear_completed();
            Ok(json!({ "ok": true, "state": backend.queue.snapshot() }))
        }
        "clear_errors" => {
            backend.queue.clear_errors();
            Ok(json!({ "ok": true, "state": backend.queue.snapshot() }))
        }
        "reset_state" => {
            backend.watcher.stop_watching();
            backend.queue.reset_state();
            Ok(json!({ "ok": true, "state": backend.queue.snapshot() }))
        }
        _ => Err(format!("Unknown action '{}'", action)),
    }
}

pub fn resolve_api_port() -> u16 {
    std::env::var("AUDIO_STUDIO_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(4895)
}

pub fn start_http_server(backend: AppBackend) {
    std::thread::spawn(move || {
        let addr = format!("127.0.0.1:{}", backend.port);
        let listener = match TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Audio Transcription Studio HTTP API failed to bind {}: {}", addr, e);
                return;
            }
        };
        for stream in listener.incoming().flatten() {
            let b = backend.clone();
            std::thread::spawn(move || {
                handle_client(stream, b);
            });
        }
    });
}
