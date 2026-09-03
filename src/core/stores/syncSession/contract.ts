import type {
  SyncErrorCategory,
  SyncErrorOrigin,
  SyncErrorStage,
  SyncRetryAction,
  SyncTerminalError,
} from "./types";

const ERROR_CATEGORIES = new Set<SyncErrorCategory>([
  "device",
  "configuration",
  "connection",
  "compatibility",
  "protocol",
  "data",
  "storage",
  "internal",
]);

const ERROR_ORIGINS = new Set<SyncErrorOrigin>([
  "mobile_ui",
  "mobile_native",
  "mobile_sync",
  "desktop_plugin",
  "desktop_cds",
]);

const ERROR_STAGES = new Set<SyncErrorStage>([
  "preflight",
  "startup",
  "connect",
  "handshake",
  "owner_metadata",
  "topic_metadata",
  "topic_validation",
  "messages",
  "finalize",
  "shutdown",
  "history",
]);

const RETRY_ACTIONS = new Set<SyncRetryAction>([
  "automatic",
  "after_user_action",
  "manual",
  "never",
]);

const MAX_FAILED_TOPIC_IDS = 8;

export const WIRE_PROTOCOL_VERSION = "1.4";
export const MAX_BUFFERED_SESSION_EVENTS = 32;

const LOCAL_ERROR_COPY: Record<
  string,
  Pick<
    SyncTerminalError,
    "category" | "origin" | "stage" | "retryAction" | "message" | "guidance"
  >
> = {
  POWER_SAVE_MODE: {
    category: "device",
    origin: "mobile_native",
    stage: "preflight",
    retryAction: "after_user_action",
    message: "系统省电模式已阻止本次同步",
    guidance: "关闭系统省电模式后再试。",
  },
  BATTERY_TOO_LOW: {
    category: "device",
    origin: "mobile_native",
    stage: "preflight",
    retryAction: "after_user_action",
    message: "当前电量不足，已暂停同步",
    guidance: "电量达到 30% 后再试。",
  },
  LISTENER_SETUP_FAILED: {
    category: "internal",
    origin: "mobile_ui",
    stage: "startup",
    retryAction: "manual",
    message: "同步面板未能正常接收进度",
    guidance: "关闭并重新打开同步面板后再试。",
  },
  INVALID_COMPLETION_EVENT: {
    category: "protocol",
    origin: "mobile_ui",
    stage: "finalize",
    retryAction: "after_user_action",
    message: "同步响应不符合 Wire 1.4 规范，已安全停止",
    guidance:
      "确认两端版本一致并重启电脑端同步插件；若仍出现，请保留最新日志。",
  },
  START_SYNC_FAILED: {
    category: "internal",
    origin: "mobile_ui",
    stage: "startup",
    retryAction: "manual",
    message: "同步组件未能正常启动",
    guidance: "重启应用后再试；若仍失败，请保留最新同步日志。",
  },
  STOP_SYNC_FAILED: {
    category: "internal",
    origin: "mobile_ui",
    stage: "shutdown",
    retryAction: "manual",
    message: "上一同步任务未能正常结束",
    guidance: "重启应用后再试；若仍失败，请保留最新同步日志。",
  },
  SYNC_ATTEMPT_FAILED: {
    category: "internal",
    origin: "mobile_ui",
    stage: "startup",
    retryAction: "manual",
    message: "同步未能完成",
    guidance: "可重试一次；若仍失败，请保留最新同步日志。",
  },
};

const safeText = (value: string, fallback: string, maxLength: number) => {
  const normalized = value.trim().slice(0, maxLength);
  return normalized || fallback;
};

export const localTerminalError = (code: string): SyncTerminalError => {
  const copy = LOCAL_ERROR_COPY[code] ?? LOCAL_ERROR_COPY.SYNC_ATTEMPT_FAILED;
  return {
    code,
    ...copy,
    failedTopicIds: [],
    logFile: null,
  };
};

const readCount = (source: Record<string, unknown>, key: string) => {
  const count = source[key];
  return typeof count === "number" && Number.isSafeInteger(count) && count >= 0
    ? count
    : null;
};

export const readSyncError = (value: unknown): SyncTerminalError | null => {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const source = value as Record<string, unknown>;
  if (
    typeof source.code !== "string" ||
    !/^[A-Z][A-Z0-9_]{0,63}$/.test(source.code) ||
    typeof source.category !== "string" ||
    !ERROR_CATEGORIES.has(source.category as SyncErrorCategory) ||
    typeof source.origin !== "string" ||
    !ERROR_ORIGINS.has(source.origin as SyncErrorOrigin) ||
    typeof source.stage !== "string" ||
    !ERROR_STAGES.has(source.stage as SyncErrorStage) ||
    typeof source.retryAction !== "string" ||
    !RETRY_ACTIONS.has(source.retryAction as SyncRetryAction) ||
    typeof source.message !== "string" ||
    source.message.trim().length === 0 ||
    source.message.length > 200 ||
    typeof source.guidance !== "string" ||
    source.guidance.trim().length === 0 ||
    source.guidance.length > 300
  ) {
    return null;
  }
  const message = safeText(source.message, "同步未能完成", 200);
  const guidance = safeText(source.guidance, "请查看同步日志后重试。", 300);
  const failedTopicIds = Array.isArray(source.failedTopicIds)
    ? source.failedTopicIds
        .filter(
          (id): id is string =>
            typeof id === "string" && id.length > 0 && id.length <= 512,
        )
        .slice(0, MAX_FAILED_TOPIC_IDS)
    : [];
  const logFile =
    typeof source.logFile === "string" &&
    source.logFile.length > 0 &&
    source.logFile.length <= 255 &&
    !source.logFile.includes("/") &&
    !source.logFile.includes("\\")
      ? source.logFile
      : null;
  return {
    code: source.code,
    category: source.category as SyncErrorCategory,
    origin: source.origin as SyncErrorOrigin,
    stage: source.stage as SyncErrorStage,
    retryAction: source.retryAction as SyncRetryAction,
    message,
    guidance,
    failedTopicIds,
    logFile,
  };
};

export const parseCommandError = (
  error: unknown,
  fallbackCode: string,
): SyncTerminalError => {
  const raw = error instanceof Error ? error.message : String(error);
  const marker = "SYNC_ERROR:";
  const markerIndex = raw.indexOf(marker);
  if (markerIndex >= 0) {
    try {
      const parsed = readSyncError(
        JSON.parse(raw.slice(markerIndex + marker.length)),
      );
      if (parsed) return parsed;
    } catch {
      // Raw transport details must never become user-facing error text.
    }
  }
  return localTerminalError(fallbackCode);
};

export const readSummary = (value: unknown) => {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const source = value as Record<string, unknown>;
  const successfulTopics = readCount(source, "successfulTopics");
  const totalTopics = readCount(source, "totalTopics");
  const failedTopics = readCount(source, "failedTopics");
  const legacyAttachmentWarnings = readCount(
    source,
    "legacyAttachmentWarnings",
  );
  const failedTopicIds = source.failedTopicIds;
  if (
    successfulTopics === null ||
    totalTopics === null ||
    failedTopics === null ||
    legacyAttachmentWarnings === null ||
    !Array.isArray(failedTopicIds) ||
    failedTopicIds.some(
      (id) => typeof id !== "string" || id.length === 0 || id.length > 512,
    ) ||
    new Set(failedTopicIds).size !== failedTopicIds.length ||
    failedTopicIds.length > failedTopics ||
    successfulTopics + failedTopics !== totalTopics
  ) {
    return null;
  }
  return {
    successfulTopics,
    totalTopics,
    failedTopics,
    legacyAttachmentWarnings,
    failedTopicIds: failedTopicIds.slice(0, MAX_FAILED_TOPIC_IDS),
  };
};

export const readProgressSummary = (payload: Record<string, unknown>) => {
  const keys = [
    "successfulTopics",
    "totalTopics",
    "failedTopics",
    "legacyAttachmentWarnings",
  ] as const;
  const values = keys.map((key) => readCount(payload, key));
  if (values.some((value) => value === null)) return null;
  const [
    successfulTopics,
    totalTopics,
    failedTopics,
    legacyAttachmentWarnings,
  ] = values as number[];
  if (successfulTopics + failedTopics > totalTopics) return null;
  return {
    successfulTopics,
    totalTopics,
    failedTopics,
    legacyAttachmentWarnings,
  };
};
