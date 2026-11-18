export type QueueStatus =
  | "waitingForCopy"
  | "queued"
  | "decoding"
  | "transcribing"
  | "exporting"
  | "completed"
  | "skippedDuplicate"
  | "failed";

export interface TranscriptSegment {
  startMs: number;
  endMs: number;
  text: string;
  language?: string | null;
}

export interface QueueItem {
  id: string;
  filePath: string;
  fileName: string;
  format: string;
  fileSize: number;
  sha256?: string | null;
  durationMs?: number | null;
  status: QueueStatus;
  progress: number;
  statusDetail?: string | null;
  detectedLanguage?: string | null;
  isSilent: boolean;
  segments: TranscriptSegment[];
  txtPath?: string | null;
  srtPath?: string | null;
  vttPath?: string | null;
  errorCode?: string | null;
  errorMessage?: string | null;
  source: string;
  createdAt: string;
  completedAt?: string | null;
}

export interface ErrorLogEntry {
  id: string;
  timestamp: string;
  filePath: string;
  errorCode: string;
  message: string;
}

export interface EngineInfo {
  whisperBin?: string | null;
  modelPath?: string | null;
  modelName?: string | null;
  ffmpegBin?: string | null;
  ready: boolean;
  offlineMode: boolean;
}

export interface AppStateSnapshot {
  items: QueueItem[];
  errorLog: ErrorLogEntry[];
  watchFolder?: string | null;
  watchActive: boolean;
  offlineMode: boolean;
  processedCount: number;
  engine: EngineInfo;
  activeWorkerId?: string | null;
}
