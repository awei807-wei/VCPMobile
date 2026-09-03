import { getVersion } from "@tauri-apps/api/app";
import type { SyncStatus, SyncSummary, SyncTerminalError } from "./types";
import { WIRE_PROTOCOL_VERSION } from "./contract";

export const sanitizeDiagnosticText = (value: string) =>
  value
    .replace(
      /Bearer\s+(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\s,;]+)/gi,
      "Bearer [redacted]",
    )
    .replace(
      /(?:sync[_-]?)?token\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s,;]+)/gi,
      "token=[redacted]",
    )
    .replace(/[A-Za-z]:[\\/][^\r\n,;]*/g, "[path]")
    .replace(/file:\/\/\/[^\r\n,;]*/gi, "file:///[path]")
    .replace(/(^|[^/])\/(?!\/)[^\r\n,;]*/g, "$1[path]");

export const buildDiagnostics = async (
  status: SyncStatus,
  sessionId: number | null,
  summary: SyncSummary,
  terminalError: SyncTerminalError | null,
) => {
  const mobileVersion = await getVersion().catch(() => "unavailable");
  const safeFailedTopicIds = [...new Set(summary.failedTopicIds)]
    .slice(0, 8)
    .map(sanitizeDiagnosticText)
    .join(", ");
  return [
    `VCP Mobile: ${mobileVersion}`,
    `Wire protocol: ${WIRE_PROTOCOL_VERSION}`,
    `Session: ${sessionId ?? "none"}`,
    `Status: ${status}`,
    `Topics: ${summary.successfulTopics}/${summary.totalTopics}`,
    `Failed topics: ${summary.failedTopics}`,
    `Failed topic IDs: ${safeFailedTopicIds || "none"}`,
    `Legacy attachment warnings: ${summary.legacyAttachmentWarnings}`,
    terminalError
      ? `Error code: ${sanitizeDiagnosticText(terminalError.code)}`
      : "Error: none",
    `Error origin: ${terminalError?.origin ?? "unavailable"}`,
    `Error stage: ${terminalError?.stage ?? "unavailable"}`,
    `Retry action: ${terminalError?.retryAction ?? "unavailable"}`,
    `Log file: ${terminalError?.logFile ?? "unavailable"}`,
  ].join("\n");
};

export const copyTextToClipboard = async (
  text: string,
  successMessage: string,
  failureMessage: string,
  pushLog: (level: string, message: string) => void,
) => {
  try {
    await navigator.clipboard.writeText(text);
    pushLog("success", successMessage);
  } catch {
    pushLog("error", failureMessage);
  }
};
