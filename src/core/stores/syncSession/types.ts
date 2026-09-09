export type SyncStatus =
  | "idle"
  | "connecting"
  | "connected"
  | "retrying"
  | "stopping"
  | "stopped"
  | "error"
  | "completed"
  | "completed_with_warnings";

export type SyncPhase =
  | "initialization"
  | "owner_metadata"
  | "topic_metadata"
  | "topic_validation"
  | "messages"
  | "finalize";

export interface SyncProgress {
  phase: SyncPhase;
  total: number;
  completed: number;
}

export interface SyncSummary {
  successfulTopics: number;
  totalTopics: number;
  failedTopics: number;
  legacyAttachmentWarnings: number;
  failedTopicIds: string[];
}

export type SyncErrorCategory =
  | "device"
  | "configuration"
  | "connection"
  | "compatibility"
  | "protocol"
  | "data"
  | "storage"
  | "internal";

export type SyncErrorOrigin =
  | "mobile_ui"
  | "mobile_native"
  | "mobile_sync"
  | "desktop_plugin"
  | "desktop_cds";

export type SyncErrorStage =
  | "preflight"
  | "startup"
  | "connect"
  | "handshake"
  | "owner_metadata"
  | "topic_metadata"
  | "topic_validation"
  | "messages"
  | "finalize"
  | "shutdown"
  | "history";

export type SyncRetryAction =
  | "automatic"
  | "after_user_action"
  | "manual"
  | "never";

export interface SyncTerminalError {
  code: string;
  category: SyncErrorCategory;
  origin: SyncErrorOrigin;
  stage: SyncErrorStage;
  retryAction: SyncRetryAction;
  message: string;
  guidance: string;
  failedTopicIds: string[];
  logFile: string | null;
}

export interface SyncLogEntry {
  id: string;
  level: string;
  message: string;
  time: string;
}

export type SessionEventKind = "status" | "progress" | "completed" | "log";

export interface BufferedSessionEvent {
  kind: SessionEventKind;
  payload: Record<string, unknown>;
}

export const emptySummary = (): SyncSummary => ({
  successfulTopics: 0,
  totalTopics: 0,
  failedTopics: 0,
  legacyAttachmentWarnings: 0,
  failedTopicIds: [],
});

export const emptyProgress = (): SyncProgress => ({
  phase: "initialization",
  total: 0,
  completed: 0,
});
