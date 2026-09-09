import type { Ref } from "vue";
import {
  localTerminalError,
  parseCommandError,
  readProgressSummary,
  readSummary,
} from "./contract";
import type {
  SessionEventKind,
  SyncPhase,
  SyncProgress,
  SyncStatus,
  SyncSummary,
  SyncTerminalError,
} from "./types";

const PHASE_LABELS: Record<SyncPhase, string> = {
  initialization: "初始化",
  owner_metadata: "所有者元数据",
  topic_metadata: "会话主题同步",
  topic_validation: "会话校验",
  messages: "历史消息同步",
  finalize: "数据收尾",
};
const INDETERMINATE_PHASES = new Set<SyncPhase>([
  "initialization",
  "owner_metadata",
  "topic_validation",
  "finalize",
]);

export interface SyncEventState {
  status: Ref<SyncStatus>;
  canDismiss: Ref<boolean>;
  activeSessionId: Ref<number | null>;
  activeAttemptId: Ref<number>;
  summary: Ref<SyncSummary>;
  terminalError: Ref<SyncTerminalError | null>;
  needsReload: Ref<boolean>;
  logs: Ref<{ id: string; level: string; message: string; time: string }[]>;
  progressData: Ref<SyncProgress>;
  pushLog: (level: string, message: string) => void;
  releaseScreen: () => void;
}

const isTerminal = (status: SyncStatus) =>
  ["error", "completed", "completed_with_warnings", "stopped"].includes(status);

function applyErrorStatus(
  state: SyncEventState,
  payload: Record<string, unknown>,
): void {
  if (
    state.status.value === "completed" ||
    state.status.value === "completed_with_warnings"
  )
    return;
  state.terminalError.value =
    typeof payload.error === "object"
      ? parseCommandError(
          `SYNC_ERROR:${JSON.stringify(payload.error)}`,
          "SYNC_ATTEMPT_FAILED",
        )
      : localTerminalError("SYNC_ATTEMPT_FAILED");
  state.pushLog("error", state.terminalError.value.message);
  const failedTopics = Math.max(
    state.summary.value.failedTopics,
    state.terminalError.value.failedTopicIds.length,
  );
  state.summary.value = {
    ...state.summary.value,
    failedTopics,
    totalTopics: Math.max(
      state.summary.value.totalTopics,
      state.summary.value.successfulTopics + failedTopics,
    ),
    failedTopicIds: state.terminalError.value.failedTopicIds,
  };
  state.status.value = "error";
  state.canDismiss.value = true;
  state.releaseScreen();
}

function applyConnectionStatus(
  state: SyncEventState,
  nextStatus: unknown,
  getLastStatus: () => string,
  setLastStatus: (status: string) => void,
): void {
  if (nextStatus === "open") {
    state.needsReload.value = true;
    state.status.value = "connected";
    state.canDismiss.value = false;
    if (getLastStatus() !== "open") {
      state.pushLog("success", "已连接电脑端，开始同步");
      setLastStatus("open");
    }
  } else if (nextStatus === "connecting") {
    state.status.value = "connecting";
    state.canDismiss.value = false;
    if (getLastStatus() !== "connecting") {
      state.pushLog("info", "正在连接电脑端同步服务");
      setLastStatus("connecting");
    }
  } else if (nextStatus === "retrying") {
    state.status.value = "retrying";
    state.canDismiss.value = false;
    state.pushLog("warning", "连接中断，正在自动重试");
  } else if (nextStatus === "stopped") {
    state.activeSessionId.value = null;
    state.status.value = "stopped";
    state.canDismiss.value = true;
    state.releaseScreen();
    state.pushLog("info", "同步已停止");
  }
}

function setCompletionError(state: SyncEventState, message: string): void {
  state.pushLog("error", message);
  state.terminalError.value = localTerminalError("INVALID_COMPLETION_EVENT");
  state.status.value = "error";
  state.canDismiss.value = true;
  state.releaseScreen();
}

type CompletionValidation =
  | { summary: SyncSummary; status: "completed" | "completed_with_warnings" }
  | { error: string };

function validateCompletion(
  payload: Record<string, unknown>,
): CompletionValidation {
  if (
    payload.status !== "completed" &&
    payload.status !== "completed_with_warnings"
  )
    return { error: "完成事件协议错误：状态非法" };
  const status = payload.status;
  const summary = readSummary(payload.summary);
  if (!summary) return { error: "完成事件协议错误：summary 非法" };
  if (
    summary.failedTopics !== 0 ||
    summary.failedTopicIds.length !== 0 ||
    (status === "completed" && summary.legacyAttachmentWarnings !== 0) ||
    (status === "completed_with_warnings" &&
      summary.legacyAttachmentWarnings === 0)
  )
    return { error: "完成事件协议错误：状态与 summary 不一致" };
  return { summary, status };
}

interface SyncEventRuntime {
  lastLoggedPhase: string;
  lastConnectionStatus: string;
  progressLineId: string | null;
}

function resetSyncEventRuntime(runtime: SyncEventRuntime): void {
  runtime.lastLoggedPhase = "";
  runtime.lastConnectionStatus = "";
  runtime.progressLineId = null;
}

function acceptSyncAttempt(state: SyncEventState, attemptId: number): boolean {
  if (attemptId === 0) return state.activeAttemptId.value === 0;
  if (state.activeAttemptId.value === 0) {
    state.activeAttemptId.value = attemptId;
    return true;
  }
  if (attemptId === state.activeAttemptId.value) return true;
  if (attemptId === state.activeAttemptId.value + 1) {
    state.activeAttemptId.value = attemptId;
    return true;
  }
  return false;
}

function updateLiveProgressLine(
  state: SyncEventState,
  runtime: SyncEventRuntime,
  phase: SyncPhase,
  completed: number,
  total: number,
): void {
  const message =
    phase === "messages"
      ? `已同步会话 ${completed}/${total}`
      : `${PHASE_LABELS[phase]}进度 ${completed}/${total}`;
  const existing = runtime.progressLineId
    ? state.logs.value.find((entry) => entry.id === runtime.progressLineId)
    : undefined;
  if (existing) {
    existing.message = message;
    return;
  }
  state.pushLog("info", message);
  runtime.progressLineId =
    state.logs.value[state.logs.value.length - 1]?.id ?? null;
}

function applyProgress(
  state: SyncEventState,
  runtime: SyncEventRuntime,
  payload: Record<string, unknown>,
): void {
  if (isTerminal(state.status.value)) return;
  const phase = payload.phase;
  if (typeof phase !== "string" || !(phase in PHASE_LABELS)) return;
  const typedPhase = phase as SyncPhase;
  const indeterminate = INDETERMINATE_PHASES.has(typedPhase);
  const rawTotal = payload.total;
  const rawCompleted = payload.completed;
  const total =
    indeterminate ||
    typeof rawTotal !== "number" ||
    !Number.isSafeInteger(rawTotal) ||
    rawTotal < 0
      ? 0
      : rawTotal;
  const completed =
    indeterminate ||
    typeof rawCompleted !== "number" ||
    !Number.isSafeInteger(rawCompleted) ||
    rawCompleted < 0
      ? 0
      : Math.min(rawCompleted, total);
  state.progressData.value = { phase: typedPhase, total, completed };
  if (typedPhase !== runtime.lastLoggedPhase) {
    runtime.progressLineId = null;
    state.pushLog("info", `开始${PHASE_LABELS[typedPhase]}`);
    runtime.lastLoggedPhase = typedPhase;
  }
  if (total > 0)
    updateLiveProgressLine(state, runtime, typedPhase, completed, total);
  const progressSummary = readProgressSummary(payload);
  if (progressSummary)
    state.summary.value = { ...state.summary.value, ...progressSummary };
}

function applyStatus(
  state: SyncEventState,
  runtime: SyncEventRuntime,
  payload: Record<string, unknown>,
): void {
  const nextStatus = payload.status;
  if (nextStatus === "error") {
    applyErrorStatus(state, payload);
    return;
  }
  if (
    state.status.value === "error" ||
    state.status.value === "completed" ||
    state.status.value === "completed_with_warnings"
  )
    return;
  applyConnectionStatus(
    state,
    nextStatus,
    () => runtime.lastConnectionStatus,
    (status) => {
      runtime.lastConnectionStatus = status;
    },
  );
}

function applyCompleted(
  state: SyncEventState,
  payload: Record<string, unknown>,
): void {
  if (
    state.status.value === "error" ||
    state.status.value === "stopped" ||
    state.status.value === "completed" ||
    state.status.value === "completed_with_warnings"
  )
    return;
  const validation = validateCompletion(payload);
  if ("error" in validation) {
    setCompletionError(state, validation.error);
    return;
  }
  state.summary.value = validation.summary;
  state.status.value = validation.status;
  state.canDismiss.value = true;
  state.needsReload.value = true;
  state.releaseScreen();
  state.pushLog(
    validation.status === "completed_with_warnings" ? "warning" : "success",
    validation.status === "completed_with_warnings"
      ? `消息已同步，${validation.summary.legacyAttachmentWarnings} 项旧附件信息无法安全识别，已跳过`
      : "同步已全部完成，关闭面板后刷新数据",
  );
}

function applySyncEvent(
  state: SyncEventState,
  runtime: SyncEventRuntime,
  kind: SessionEventKind,
  payload: Record<string, unknown>,
): void {
  if (kind === "log") {
    if (payload.audience !== "operator" || typeof payload.message !== "string")
      return;
    const message = payload.message.trim();
    if (message)
      state.pushLog(
        typeof payload.level === "string" ? payload.level : "info",
        message,
      );
    return;
  }
  if (kind === "progress") applyProgress(state, runtime, payload);
  else if (kind === "status") applyStatus(state, runtime, payload);
  else applyCompleted(state, payload);
}

export const createSyncEventHandler = (state: SyncEventState) => {
  const runtime: SyncEventRuntime = {
    lastLoggedPhase: "",
    lastConnectionStatus: "",
    progressLineId: null,
  };
  return {
    reset: () => resetSyncEventRuntime(runtime),
    acceptAttempt: (attemptId: number) => acceptSyncAttempt(state, attemptId),
    apply: (kind: SessionEventKind, payload: Record<string, unknown>) =>
      applySyncEvent(state, runtime, kind, payload),
  };
};
