import { nextTick, type Ref } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import type { Attachment } from "../types/chat";
import { parseStoredAttachmentResult } from "./attachmentStoredResult";

export type AttachmentPickerMode = "camera" | "gallery" | "file";

interface NotificationSink {
  addNotification(notification: {
    type: "warning" | "error";
    title: string;
    message: string;
    toastOnly: boolean;
  }): void;
}

interface NativePickedFile {
  path?: string;
  name?: string;
  size?: number;
  mime?: string;
  hash?: string;
  thumbnailPath?: string;
}

interface NativePickerDependencies {
  stagedAttachments: Ref<Attachment[]>;
  notificationStore: NotificationSink;
}

const NATIVE_PICKER_TIMEOUT_MS = 5 * 60 * 1_000;

function createStableId(): string {
  return `att_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
}

function removeAttachment(
  stagedAttachments: Ref<Attachment[]>,
  stableId: string,
) {
  const index = stagedAttachments.value.findIndex(
    (attachment) => attachment.id === stableId,
  );
  if (index !== -1) stagedAttachments.value.splice(index, 1);
}

function createLoadingAttachment(
  stableId: string,
  data: { name?: string; size?: number; mime?: string; progress?: number },
): Attachment {
  return {
    id: stableId,
    type: data.mime || "application/octet-stream",
    src: "",
    name: data.name || "文件",
    size: data.size || 0,
    progress: data.progress,
    status: "loading",
  };
}

function upsertNativeProgress(
  stagedAttachments: Ref<Attachment[]>,
  stableId: string,
  detail: {
    name?: string;
    size?: number;
    mime?: string;
    total?: number;
    progress?: number;
  },
) {
  const progress = Math.round((detail.progress || 0) * 0.9);
  const index = stagedAttachments.value.findIndex(
    (attachment) => attachment.id === stableId,
  );
  if (index !== -1) {
    stagedAttachments.value[index].progress = progress;
    return;
  }
  if (detail.name) {
    stagedAttachments.value.unshift(
      createLoadingAttachment(stableId, {
        name: detail.name,
        size: detail.total,
        mime: detail.mime,
        progress,
      }),
    );
  }
}

function ensureNativeLoadingCard(
  stagedAttachments: Ref<Attachment[]>,
  stableId: string,
  picked: NativePickedFile,
) {
  const index = stagedAttachments.value.findIndex(
    (attachment) => attachment.id === stableId,
  );
  if (index === -1) {
    stagedAttachments.value.unshift(
      createLoadingAttachment(stableId, {
        name: picked.name,
        size: picked.size,
        mime: picked.mime,
        progress: 90,
      }),
    );
    return;
  }
  stagedAttachments.value[index].progress = 90;
}

function waitForNativePicker(
  mode: AttachmentPickerMode,
  stableId: string,
  stagedAttachments: Ref<Attachment[]>,
): Promise<NativePickedFile | undefined> {
  return new Promise((resolve, reject) => {
    let settled = false;
    let timer: number | undefined;
    const cleanup = () => {
      window.removeEventListener("vcp-mobile-file-start", handleStart);
      window.removeEventListener("vcp-mobile-file-progress", handleProgress);
      window.removeEventListener("vcp-mobile-file-picked", handlePicked);
      if (timer !== undefined) window.clearTimeout(timer);
    };
    const settle = (callback: () => void) => {
      if (settled) return;
      settled = true;
      cleanup();
      callback();
    };
    const handleStart = (event: Event) => {
      const detail = (event as CustomEvent).detail || {};
      if (!settled) {
        stagedAttachments.value.unshift(
          createLoadingAttachment(stableId, detail),
        );
      }
    };
    const handleProgress = (event: Event) => {
      if (settled) return;
      upsertNativeProgress(
        stagedAttachments,
        stableId,
        (event as CustomEvent).detail || {},
      );
    };
    const handlePicked = (event: Event) => {
      settle(() => resolve((event as CustomEvent).detail));
    };

    window.addEventListener("vcp-mobile-file-start", handleStart);
    window.addEventListener("vcp-mobile-file-progress", handleProgress);
    window.addEventListener("vcp-mobile-file-picked", handlePicked);
    timer = window.setTimeout(() => {
      settle(() => reject(new Error("原生文件选择器超时，请重试")));
    }, NATIVE_PICKER_TIMEOUT_MS);

    invoke<NativePickedFile>("plugin:vcp-mobile|pick_file", { mode })
      .then((result) => settle(() => resolve(result)))
      .catch((error) => settle(() => reject(error)));
  });
}

function showUnsupportedNotification(
  notificationStore: NotificationSink,
  error: unknown,
) {
  const message = error instanceof Error ? error.message : String(error);
  notificationStore.addNotification({
    type: "warning",
    title: "不支持的附件格式",
    message: message.startsWith("❌") ? message : `❌ ${message}`,
    toastOnly: false,
  });
}

async function checkNativeSupport(
  picked: NativePickedFile,
  notificationStore: NotificationSink,
): Promise<boolean> {
  if (!picked.name) return true;
  try {
    await invoke("check_attachment_support", { originalName: picked.name });
    return true;
  } catch (error) {
    showUnsupportedNotification(notificationStore, error);
    return false;
  }
}

function applyNativePreview(
  stagedAttachments: Ref<Attachment[]>,
  stableId: string,
  picked: NativePickedFile,
) {
  const source =
    picked.thumbnailPath ||
    (picked.mime?.startsWith("image/") ? picked.path : undefined);
  if (!source) return;
  const index = stagedAttachments.value.findIndex(
    (attachment) => attachment.id === stableId,
  );
  if (index !== -1) stagedAttachments.value[index].src = convertFileSrc(source);
}

function applyNativeResult(
  stagedAttachments: Ref<Attachment[]>,
  stableId: string,
  finalData: unknown,
) {
  const stored = parseStoredAttachmentResult(finalData);
  const index = stagedAttachments.value.findIndex(
    (attachment) => attachment.id === stableId,
  );
  if (index === -1) return;
  stagedAttachments.value[index] = {
    ...stagedAttachments.value[index],
    ...stored,
    status: "done",
    progress: undefined,
  };
}

function showNativeFailure(
  notificationStore: NotificationSink,
  error: unknown,
) {
  const message = error instanceof Error ? error.message : String(error);
  const cancelled = /cancel/i.test(message) || message === "Cancelled";
  if (cancelled) return;
  notificationStore.addNotification({
    type: "warning",
    title: "选取附件失败",
    message: `❌ ${message}`,
    toastOnly: true,
  });
}

/** 调用 Android 文件选择器，并将选中的文件注册到本地附件仓库。 */
export async function pickNativeAttachment(
  { stagedAttachments, notificationStore }: NativePickerDependencies,
  mode: AttachmentPickerMode,
): Promise<void> {
  const stableId = createStableId();
  try {
    const picked = await waitForNativePicker(mode, stableId, stagedAttachments);
    if (!picked?.path) {
      removeAttachment(stagedAttachments, stableId);
      return;
    }
    if (!(await checkNativeSupport(picked, notificationStore))) {
      removeAttachment(stagedAttachments, stableId);
      return;
    }

    ensureNativeLoadingCard(stagedAttachments, stableId, picked);
    applyNativePreview(stagedAttachments, stableId, picked);
    await nextTick();
    window.dispatchEvent(new Event("resize"));
    const finalData = await invoke("register_local_file", {
      localPath: picked.path,
      originalName: picked.name,
      mimeType: picked.mime || "application/octet-stream",
      thumbnailPath: picked.thumbnailPath || null,
      stableId,
      expectedHash: picked.hash || null,
    });
    applyNativeResult(stagedAttachments, stableId, finalData);
  } catch (error) {
    removeAttachment(stagedAttachments, stableId);
    console.error("[附件仓库] 原生文件选取或注册失败:", error);
    showNativeFailure(notificationStore, error);
  }
}
