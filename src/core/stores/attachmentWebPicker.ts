import { nextTick, type Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { Attachment } from "../types/chat";
import { uploadFileWithXhr, type VcpUploadEndpoint } from "./attachmentUpload";
import type { AttachmentPickerMode } from "./attachmentNativePicker";
import { parseStoredAttachmentResult } from "./attachmentStoredResult";

interface NotificationSink {
  addNotification(notification: {
    type: "warning" | "error";
    title: string;
    message: string;
    toastOnly: boolean;
  }): void;
}

interface WebPickerDependencies {
  stagedAttachments: Ref<Attachment[]>;
  notificationStore: NotificationSink;
}

const SMALL_FILE_LIMIT = 2 * 1024 * 1024;
const IMAGE_SIZE_LIMIT = 10 * 1024 * 1024;
const IMAGE_DIMENSION_LIMIT = 8_192;

function createStableId(): string {
  return `att_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
}

function notifyUnsupported(
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

function notifyUploadFailure(
  notificationStore: NotificationSink,
  error: unknown,
) {
  const message = error instanceof Error ? error.message : String(error);
  notificationStore.addNotification({
    type: "warning",
    title: "附件上传失败",
    message: message || "上传未完成，请重试。",
    toastOnly: true,
  });
}

function checkImageDimensions(
  file: File,
): Promise<{ width: number; height: number }> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    const url = URL.createObjectURL(file);
    image.onload = () => {
      URL.revokeObjectURL(url);
      resolve({ width: image.naturalWidth, height: image.naturalHeight });
    };
    image.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("无法读取图片尺寸"));
    };
    image.src = url;
  });
}

async function validateWebFile(
  file: File,
  notificationStore: NotificationSink,
): Promise<boolean> {
  try {
    await invoke("check_attachment_support", { originalName: file.name });
  } catch (error) {
    notifyUnsupported(notificationStore, error);
    return false;
  }

  const extension = file.name.split(".").pop()?.toLowerCase() || "";
  const isGif = extension === "gif" || file.type === "image/gif";
  const isImage = file.type.startsWith("image/");
  if (isImage && !isGif && file.size > IMAGE_SIZE_LIMIT) {
    notificationStore.addNotification({
      type: "warning",
      title: "图片过大",
      message: "图片过大（>10MB），请压缩后重试。",
      toastOnly: true,
    });
    return false;
  }
  if (isImage && !isGif) {
    try {
      const dimensions = await checkImageDimensions(file);
      if (
        dimensions.width > IMAGE_DIMENSION_LIMIT ||
        dimensions.height > IMAGE_DIMENSION_LIMIT
      ) {
        notificationStore.addNotification({
          type: "warning",
          title: "分辨率过高",
          message: "图片分辨率过高（>8K），请压缩后重试。",
          toastOnly: true,
        });
        return false;
      }
    } catch (error) {
      console.warn("[附件仓库] 检查图片尺寸失败:", error);
    }
  }
  return true;
}

function configureInput(input: HTMLInputElement, mode: AttachmentPickerMode) {
  input.type = "file";
  input.multiple = false;
  if (mode === "camera") {
    input.accept = "image/*";
    input.setAttribute("capture", "environment");
  } else if (mode === "gallery") {
    input.accept = "image/*";
  } else {
    input.accept = "*/*";
  }
}

function stageWebFile(
  stagedAttachments: Ref<Attachment[]>,
  file: File,
  stableId: string,
): string {
  const blobUrl = URL.createObjectURL(file);
  stagedAttachments.value.unshift({
    id: stableId,
    type: file.type || "application/octet-stream",
    src: blobUrl,
    name: file.name,
    size: file.size,
    status: "loading",
  });
  return blobUrl;
}

function setAttachmentStatus(
  stagedAttachments: Ref<Attachment[]>,
  stableId: string,
  status: "loading" | "processing",
) {
  const index = stagedAttachments.value.findIndex(
    (attachment) => attachment.id === stableId,
  );
  if (index !== -1) stagedAttachments.value[index].status = status;
}

function updateWebProgress(
  stagedAttachments: Ref<Attachment[]>,
  stableId: string,
  percent: number,
  lastUpdate: { at: number },
) {
  const now = Date.now();
  if (now - lastUpdate.at < 33) return;
  lastUpdate.at = now;
  const index = stagedAttachments.value.findIndex(
    (attachment) => attachment.id === stableId,
  );
  if (index === -1) return;
  if (percent >= 99) {
    setAttachmentStatus(stagedAttachments, stableId, "processing");
    stagedAttachments.value[index].progress = undefined;
  } else {
    stagedAttachments.value[index].progress = percent;
  }
}

async function storeSmallFile(
  file: File,
  stagedAttachments: Ref<Attachment[]>,
  stableId: string,
) {
  const bytes = new Uint8Array(await file.arrayBuffer());
  setAttachmentStatus(stagedAttachments, stableId, "processing");
  return invoke("store_file", {
    originalName: file.name,
    fileBytes: bytes,
    mimeType: file.type || "application/octet-stream",
  });
}

async function storeLargeFile(
  file: File,
  stagedAttachments: Ref<Attachment[]>,
  stableId: string,
) {
  const endpoint = await invoke<VcpUploadEndpoint>("prepare_vcp_upload", {
    metadata: {
      name: file.name,
      mime: file.type || "application/octet-stream",
      size: file.size,
    },
  });
  if (!endpoint?.url || !endpoint.token) {
    throw new Error("上传服务未返回有效传输端点");
  }
  const lastUpdate = { at: 0 };
  return uploadFileWithXhr(file, endpoint, ({ percent }) => {
    updateWebProgress(stagedAttachments, stableId, percent, lastUpdate);
  });
}

function applyWebResult(
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

function removeWebAttachment(
  stagedAttachments: Ref<Attachment[]>,
  stableId: string,
) {
  const index = stagedAttachments.value.findIndex(
    (attachment) => attachment.id === stableId,
  );
  if (index !== -1) stagedAttachments.value.splice(index, 1);
}

async function processWebFile(
  file: File,
  dependencies: WebPickerDependencies,
): Promise<void> {
  const { stagedAttachments, notificationStore } = dependencies;
  const stableId = createStableId();
  if (!(await validateWebFile(file, notificationStore))) return;
  const blobUrl = stageWebFile(stagedAttachments, file, stableId);
  try {
    await nextTick();
    window.dispatchEvent(new Event("resize"));
    const finalData =
      file.size < SMALL_FILE_LIMIT
        ? await storeSmallFile(file, stagedAttachments, stableId)
        : await storeLargeFile(file, stagedAttachments, stableId);
    applyWebResult(stagedAttachments, stableId, finalData);
  } catch (error) {
    removeWebAttachment(stagedAttachments, stableId);
    console.error("[附件仓库] Web 附件上传失败:", error);
    notifyUploadFailure(notificationStore, error);
  } finally {
    URL.revokeObjectURL(blobUrl);
  }
}

/** 打开浏览器文件选择器，并确保失败或取消的文件不会留在暂存列表中。 */
export function pickWebAttachment(
  dependencies: WebPickerDependencies,
  mode: AttachmentPickerMode,
): Promise<void> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    configureInput(input, mode);
    input.onchange = async (event) => {
      try {
        const file = (event.target as HTMLInputElement).files?.[0];
        if (file) await processWebFile(file, dependencies);
      } finally {
        resolve();
      }
    };
    input.oncancel = () => resolve();
    input.click();
  });
}
