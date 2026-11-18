import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  AlertTriangle,
  AudioLines,
  CheckCircle2,
  Clock,
  CopyCheck,
  FileAudio,
  FileText,
  FolderOpen,
  FolderSync,
  Languages,
  Loader2,
  PauseCircle,
  Play,
  RefreshCw,
  ShieldCheck,
  Trash2,
  Upload,
} from "lucide-react";
import type { AppStateSnapshot, QueueItem, QueueStatus } from "./types";

function formatDuration(ms?: number | null): string {
  if (ms === undefined || ms === null) return "—";
  const totalSec = Math.floor(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  if (h > 0) return `${h}h ${m}m ${s}s`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

function formatTimestamp(ms: number): string {
  const totalSec = Math.floor(ms / 1000);
  const millis = ms % 1000;
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  const pad2 = (n: number) => String(n).padStart(2, "0");
  const pad3 = (n: number) => String(n).padStart(3, "0");
  if (h > 0) {
    return `${pad2(h)}:${pad2(m)}:${pad2(s)}.${pad3(millis)}`;
  }
  return `${pad2(m)}:${pad2(s)}.${pad3(millis)}`;
}

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let idx = 0;
  let val = bytes;
  while (val >= 1024 && idx < units.length - 1) {
    val /= 1024;
    idx++;
  }
  return `${val.toFixed(idx === 0 ? 0 : 1)} ${units[idx]}`;
}

function statusLabel(status: QueueStatus): string {
  switch (status) {
    case "waitingForCopy":
      return "Waiting Copy";
    case "queued":
      return "Queued";
    case "decoding":
      return "Decoding";
    case "transcribing":
      return "Transcribing";
    case "exporting":
      return "Exporting";
    case "completed":
      return "Completed";
    case "skippedDuplicate":
      return "Skipped (Duplicate)";
    case "failed":
      return "Failed";
  }
}

function isInProgress(status: QueueStatus): boolean {
  return (
    status === "waitingForCopy" ||
    status === "queued" ||
    status === "decoding" ||
    status === "transcribing" ||
    status === "exporting"
  );
}

export default function App() {
  const [snapshot, setSnapshot] = useState<AppStateSnapshot | null>(null);
  const [apiPort, setApiPort] = useState<number>(4895);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [rightTab, setRightTab] = useState<"transcript" | "errors">("transcript");
  const [exportTab, setExportTab] = useState<"segments" | "txt" | "srt" | "vtt">("segments");
  const [rawExports, setRawExports] = useState<{ txt: string; srt: string; vtt: string }>({
    txt: "",
    srt: "",
    vtt: "",
  });
  const [isDragging, setIsDragging] = useState(false);
  const [playbackRate, setPlaybackRate] = useState<number>(1);
  const [currentTimeMs, setCurrentTimeMs] = useState<number>(0);

  const audioRef = useRef<HTMLAudioElement | null>(null);
  const videoRef = useRef<HTMLVideoElement | null>(null);

  const refreshState = async () => {
    try {
      const s = await invoke<AppStateSnapshot>("get_app_state");
      setSnapshot(s);
    } catch {
      try {
        const res = await fetch(`http://127.0.0.1:${apiPort}/state`);
        if (res.ok) {
          setSnapshot(await res.json());
        }
      } catch {
        // Backend starting up
      }
    }
  };

  useEffect(() => {
    invoke<number>("get_api_port")
      .then((p) => setApiPort(p))
      .catch(() => {});

    refreshState();

    let unlistenState: (() => void) | null = null;
    let unlistenDrop: (() => void) | null = null;

    listen<AppStateSnapshot>("state-updated", (event) => {
      setSnapshot(event.payload);
    })
      .then((fn) => {
        unlistenState = fn;
      })
      .catch(() => {});

    try {
      getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type === "over") {
            setIsDragging(true);
          } else if (event.payload.type === "leave") {
            setIsDragging(false);
          } else if (event.payload.type === "drop") {
            setIsDragging(false);
            const paths = event.payload.paths;
            if (paths && paths.length > 0) {
              enqueuePaths(paths);
            }
          }
        })
        .then((fn) => {
          unlistenDrop = fn;
        })
        .catch(() => {});
    } catch {
      // Browser preview fallback
    }

    const timer = setInterval(refreshState, 1500);
    return () => {
      clearInterval(timer);
      if (unlistenState) unlistenState();
      if (unlistenDrop) unlistenDrop();
    };
  }, []);

  const items = snapshot?.items ?? [];
  const errorLog = snapshot?.errorLog ?? [];

  const selectedItem: QueueItem | null = useMemo(() => {
    if (!items.length) return null;
    const found = items.find((i) => i.id === selectedId);
    return found ?? items[items.length - 1];
  }, [items, selectedId]);

  useEffect(() => {
    if (!selectedItem || selectedItem.status !== "completed") {
      setRawExports({ txt: "", srt: "", vtt: "" });
      return;
    }
    const loadText = async (path?: string | null) => {
      if (!path) return "";
      try {
        return await invoke<string>("read_export_text", { path });
      } catch {
        return "";
      }
    };
    Promise.all([
      loadText(selectedItem.txtPath),
      loadText(selectedItem.srtPath),
      loadText(selectedItem.vttPath),
    ]).then(([txt, srt, vtt]) => {
      setRawExports({ txt, srt, vtt });
    });
  }, [selectedItem?.id, selectedItem?.status, selectedItem?.completedAt]);

  useEffect(() => {
    const media = selectedItem?.format === "mp4" ? videoRef.current : audioRef.current;
    if (media) {
      media.playbackRate = playbackRate;
    }
  }, [playbackRate, selectedItem?.id]);

  const enqueuePaths = async (paths: string[], force = false) => {
    if (!paths.length) return;
    try {
      const added = await invoke<QueueItem[]>("enqueue_files", { paths, force });
      if (added.length > 0) {
        setSelectedId(added[added.length - 1].id);
        setRightTab("transcript");
      }
      await refreshState();
    } catch (err) {
      console.error("Failed enqueueing files:", err);
    }
  };

  const handleSelectFiles = async () => {
    const picked = await open({
      multiple: true,
      filters: [
        {
          name: "Audio & Video (MP3, WAV, M4A, MP4)",
          extensions: ["mp3", "wav", "m4a", "mp4"],
        },
      ],
    });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    await enqueuePaths(paths);
  };

  const handleSelectWatchFolder = async () => {
    const folder = await open({
      directory: true,
      multiple: false,
      title: "Select Watch Folder for Automatic Transcription",
    });
    if (!folder || Array.isArray(folder)) return;
    try {
      const updated = await invoke<AppStateSnapshot>("set_watch_folder", { folder });
      setSnapshot(updated);
    } catch (err) {
      await refreshState();
      setRightTab("errors");
      console.error("Watch folder error:", err);
    }
  };

  const handleToggleWatch = async () => {
    if (!snapshot?.watchFolder) {
      await handleSelectWatchFolder();
      return;
    }
    if (snapshot.watchActive) {
      const updated = await invoke<AppStateSnapshot>("stop_watch_folder");
      setSnapshot(updated);
    } else {
      const updated = await invoke<AppStateSnapshot>("set_watch_folder", {
        folder: snapshot.watchFolder,
      });
      setSnapshot(updated);
    }
  };

  const handleSeekTo = (ms: number) => {
    const media = selectedItem?.format === "mp4" ? videoRef.current : audioRef.current;
    if (!media) return;
    media.currentTime = ms / 1000;
    media.play().catch(() => {});
  };

  const handleRevealPath = async (filePath?: string | null) => {
    if (!filePath) return;
    try {
      await openPath(filePath);
    } catch (err) {
      console.error("openPath failed:", err);
    }
  };

  const stats = useMemo(() => {
    let queued = 0;
    let active = 0;
    let completed = 0;
    let duplicates = 0;
    let failed = 0;
    for (const item of items) {
      if (item.status === "completed") completed++;
      else if (item.status === "skippedDuplicate") duplicates++;
      else if (item.status === "failed") failed++;
      else if (item.status === "queued" || item.status === "waitingForCopy") queued++;
      else active++;
    }
    return { queued, active, completed, duplicates, failed };
  }, [items]);

  const mediaUrl =
    selectedItem && selectedItem.filePath
      ? `http://127.0.0.1:${apiPort}/media?path=${encodeURIComponent(selectedItem.filePath)}`
      : "";

  return (
    <div className="studio-shell">
      <header className="studio-header">
        <div className="brand-block">
          <div className="brand-mark">
            <AudioLines size={20} />
          </div>
          <div>
            <h1>Audio Transcription Studio</h1>
            <p className="subtitle">
              Local Whisper speech-to-text for macOS &amp; Windows • Zero cloud uploads
            </p>
          </div>
        </div>

        <div className="header-badges">
          <span className="pill pill-offline" title="100% offline processing without API keys">
            <ShieldCheck size={14} />
            Offline • Local Only
          </span>
          <span
            className={`pill ${snapshot?.engine.ready ? "pill-ready" : "pill-warn"}`}
            title={snapshot?.engine.modelPath ?? "Whisper model path"}
          >
            {snapshot?.engine.ready
              ? `Whisper (${snapshot.engine.modelName ?? "ready"})`
              : "Model / whisper-cli missing"}
          </span>
        </div>
      </header>

      <main className="studio-grid">
        {/* Left Sidebar: Upload, Watch Folder, Queue */}
        <section className="left-column">
          <div
            className={`dropzone ${isDragging ? "dropzone-active" : ""}`}
            onDragOver={(e) => {
              e.preventDefault();
              setIsDragging(true);
            }}
            onDragLeave={() => setIsDragging(false)}
            onDrop={(e) => {
              e.preventDefault();
              setIsDragging(false);
            }}
          >
            <Upload size={26} className="drop-icon" />
            <div className="drop-copy">
              <strong>Drop MP3, WAV, M4A, or MP4 files here</strong>
              <span>Auto-detects language and saves TXT, SRT, and VTT alongside source files</span>
            </div>
            <button type="button" className="btn-primary" onClick={handleSelectFiles}>
              Select Media Files
            </button>
          </div>

          <div className="watch-card">
            <div className="watch-header">
              <div className="watch-title">
                <FolderSync size={17} />
                <span>Watch Folder</span>
              </div>
              <span
                className={`status-dot ${snapshot?.watchActive ? "dot-live" : "dot-paused"}`}
              >
                {snapshot?.watchActive ? "Watching" : "Off"}
              </span>
            </div>

            <div className="watch-path">
              {snapshot?.watchFolder ? (
                <code title={snapshot.watchFolder}>{snapshot.watchFolder}</code>
              ) : (
                <span className="muted">No folder selected yet</span>
              )}
            </div>

            <p className="watch-note">
              Automatically queues new MP3/WAV/M4A/MP4 files after copy completion and skips
              duplicates.
            </p>

            <div className="watch-actions">
              <button type="button" className="btn-secondary" onClick={handleSelectWatchFolder}>
                <FolderOpen size={14} />
                Choose Folder
              </button>
              {snapshot?.watchFolder && (
                <button type="button" className="btn-secondary" onClick={handleToggleWatch}>
                  {snapshot.watchActive ? (
                    <>
                      <PauseCircle size={14} /> Pause
                    </>
                  ) : (
                    <>
                      <Play size={14} /> Resume
                    </>
                  )}
                </button>
              )}
            </div>
          </div>

          <div className="queue-panel">
            <div className="queue-header">
              <div>
                <h2>Processing Queue ({items.length})</h2>
                <span className="queue-stats">
                  {stats.active > 0 && `${stats.active} active • `}
                  {stats.queued > 0 && `${stats.queued} queued • `}
                  {stats.completed} done • {stats.duplicates} skipped • {stats.failed} errors
                </span>
              </div>
              {items.length > 0 && (
                <button
                  type="button"
                  className="btn-ghost"
                  onClick={() => invoke("clear_completed").then(refreshState)}
                  title="Clear finished & skipped items"
                >
                  <Trash2 size={14} /> Clear Done
                </button>
              )}
            </div>

            <div className="queue-list">
              {items.length === 0 ? (
                <div className="empty-queue">
                  <FileAudio size={28} />
                  <p>Drop files or enable Watch Folder to start transcribing.</p>
                </div>
              ) : (
                items
                  .slice()
                  .reverse()
                  .map((item) => (
                    <div
                      key={item.id}
                      className={`queue-item ${
                        selectedItem?.id === item.id ? "queue-item-selected" : ""
                      }`}
                      onClick={() => {
                        setSelectedId(item.id);
                        setRightTab("transcript");
                      }}
                    >
                      <div className="qi-top">
                        <span className="qi-name" title={item.filePath}>
                          {item.fileName}
                        </span>
                        <span className={`badge badge-${item.status}`}>
                          {isInProgress(item.status) && (
                            <Loader2 size={11} className="spin" />
                          )}
                          {item.status === "completed" && <CheckCircle2 size={11} />}
                          {item.status === "skippedDuplicate" && <CopyCheck size={11} />}
                          {item.status === "failed" && <AlertTriangle size={11} />}
                          {statusLabel(item.status)}
                        </span>
                      </div>

                      <div className="qi-meta">
                        <span>{item.format.toUpperCase()}</span>
                        <span>{formatBytes(item.fileSize)}</span>
                        {item.durationMs !== undefined && item.durationMs !== null && (
                          <span>
                            <Clock size={11} /> {formatDuration(item.durationMs)}
                          </span>
                        )}
                        {item.detectedLanguage && (
                          <span className="lang-pill">
                            <Languages size={11} /> {item.detectedLanguage.toUpperCase()}
                          </span>
                        )}
                      </div>

                      {isInProgress(item.status) && (
                        <div className="progress-track">
                          <div
                            className="progress-fill"
                            style={{ width: `${Math.max(5, item.progress)}%` }}
                          />
                        </div>
                      )}

                      {item.statusDetail && (
                        <div className="qi-detail">{item.statusDetail}</div>
                      )}

                      <div className="qi-actions" onClick={(e) => e.stopPropagation()}>
                        {item.status === "failed" && (
                          <button
                            type="button"
                            className="mini-btn"
                            onClick={() =>
                              invoke("retry_item", { id: item.id }).then(refreshState)
                            }
                          >
                            <RefreshCw size={11} /> Retry
                          </button>
                        )}
                        {(item.status === "completed" || item.status === "skippedDuplicate") && (
                          <button
                            type="button"
                            className="mini-btn"
                            onClick={() =>
                              invoke("force_retranscribe", { id: item.id }).then(refreshState)
                            }
                          >
                            <RefreshCw size={11} /> Re-transcribe
                          </button>
                        )}
                      </div>
                    </div>
                  ))
              )}
            </div>
          </div>
        </section>

        {/* Right Panel: Player, Timestamped Transcript, Error Log */}
        <section className="right-column">
          <div className="right-tabs">
            <button
              type="button"
              className={`tab-btn ${rightTab === "transcript" ? "tab-active" : ""}`}
              onClick={() => setRightTab("transcript")}
            >
              <FileText size={15} /> Transcript &amp; Player
            </button>
            <button
              type="button"
              className={`tab-btn ${rightTab === "errors" ? "tab-active" : ""}`}
              onClick={() => setRightTab("errors")}
            >
              <AlertTriangle size={15} /> Error Log ({errorLog.length})
            </button>
          </div>

          {rightTab === "errors" ? (
            <div className="error-log-panel">
              <div className="panel-subheader">
                <h3>Processing &amp; Access Error Log</h3>
                {errorLog.length > 0 && (
                  <button
                    type="button"
                    className="btn-ghost"
                    onClick={() => invoke("clear_errors").then(refreshState)}
                  >
                    Clear Log
                  </button>
                )}
              </div>
              {errorLog.length === 0 ? (
                <div className="empty-viewer">
                  <CheckCircle2 size={30} />
                  <p>No errors recorded.</p>
                </div>
              ) : (
                <div className="error-list">
                  {errorLog.map((entry) => (
                    <div key={entry.id} className="error-row">
                      <div className="error-row-head">
                        <span className="error-code">{entry.errorCode}</span>
                        <span className="error-time">
                          {new Date(entry.timestamp).toLocaleString()}
                        </span>
                      </div>
                      <div className="error-file">{entry.filePath}</div>
                      <div className="error-msg">{entry.message}</div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          ) : !selectedItem ? (
            <div className="empty-viewer">
              <AudioLines size={36} />
              <h3>Ready to transcribe</h3>
              <p>
                Select or drop an MP3, WAV, M4A, or MP4 file to inspect timestamped text and play
                audio here.
              </p>
            </div>
          ) : (
            <div className="viewer-content">
              <div className="viewer-head">
                <div>
                  <h2>{selectedItem.fileName}</h2>
                  <div className="viewer-meta">
                    <span>{selectedItem.format.toUpperCase()}</span>
                    <span>•</span>
                    <span>Duration: {formatDuration(selectedItem.durationMs)}</span>
                    <span>•</span>
                    <span>
                      Language: {selectedItem.detectedLanguage ?? "detecting..."}
                    </span>
                  </div>
                </div>
                <div className="speed-group">
                  {[1, 1.25, 1.5, 2].map((rate) => (
                    <button
                      key={rate}
                      type="button"
                      className={`speed-btn ${playbackRate === rate ? "speed-active" : ""}`}
                      onClick={() => setPlaybackRate(rate)}
                    >
                      {rate}x
                    </button>
                  ))}
                </div>
              </div>

              <div className="player-box">
                {selectedItem.format === "mp4" ? (
                  <video
                    ref={videoRef}
                    key={mediaUrl}
                    src={mediaUrl}
                    controls
                    className="video-player"
                    onTimeUpdate={(e) =>
                      setCurrentTimeMs(Math.floor(e.currentTarget.currentTime * 1000))
                    }
                  />
                ) : (
                  <audio
                    ref={audioRef}
                    key={mediaUrl}
                    src={mediaUrl}
                    controls
                    className="audio-player"
                    onTimeUpdate={(e) =>
                      setCurrentTimeMs(Math.floor(e.currentTarget.currentTime * 1000))
                    }
                  />
                )}
              </div>

              {selectedItem.status === "completed" && (
                <div className="exports-bar">
                  <span className="exports-label">Saved nearby:</span>
                  {[
                    { label: "TXT", path: selectedItem.txtPath },
                    { label: "SRT", path: selectedItem.srtPath },
                    { label: "VTT", path: selectedItem.vttPath },
                  ].map(
                    (exp) =>
                      exp.path && (
                        <button
                          key={exp.label}
                          type="button"
                          className="export-chip"
                          onClick={() => handleRevealPath(exp.path)}
                          title={exp.path}
                        >
                          {exp.label}
                        </button>
                      )
                  )}
                </div>
              )}

              <div className="subtabs">
                {(["segments", "txt", "srt", "vtt"] as const).map((tab) => (
                  <button
                    key={tab}
                    type="button"
                    className={`subtab-btn ${exportTab === tab ? "subtab-active" : ""}`}
                    onClick={() => setExportTab(tab)}
                  >
                    {tab === "segments" ? "Timestamped Segments" : tab.toUpperCase()}
                  </button>
                ))}
              </div>

              {exportTab === "segments" ? (
                <div className="segments-list">
                  {selectedItem.status === "failed" ? (
                    <div className="item-error-banner">
                      <strong>{selectedItem.errorCode ?? "ERROR"}:</strong>{" "}
                      {selectedItem.errorMessage}
                    </div>
                  ) : selectedItem.isSilent ? (
                    <div className="silence-banner">
                      Silence detected (no spoken words). Empty TXT, SRT, and WEBVTT files were
                      exported cleanly.
                    </div>
                  ) : selectedItem.segments.length === 0 ? (
                    <div className="empty-segments">
                      {isInProgress(selectedItem.status)
                        ? selectedItem.statusDetail ?? "Processing audio..."
                        : selectedItem.status === "skippedDuplicate"
                          ? "Duplicate file skipped because identical audio was already transcribed."
                          : "No speech segments extracted."}
                    </div>
                  ) : (
                    selectedItem.segments.map((seg, idx) => {
                      const active =
                        currentTimeMs >= seg.startMs && currentTimeMs <= seg.endMs;
                      return (
                        <button
                          key={`${seg.startMs}-${idx}`}
                          type="button"
                          className={`segment-row ${active ? "segment-active" : ""}`}
                          onClick={() => handleSeekTo(seg.startMs)}
                        >
                          <span className="seg-time">
                            {formatTimestamp(seg.startMs)} → {formatTimestamp(seg.endMs)}
                          </span>
                          <span className="seg-text">{seg.text}</span>
                        </button>
                      );
                    })
                  )}
                </div>
              ) : (
                <pre className="raw-export-view">
                  {rawExports[exportTab] || "(No content yet or empty file)"}
                </pre>
              )}
            </div>
          )}
        </section>
      </main>
    </div>
  );
}
