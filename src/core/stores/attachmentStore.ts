import { defineStore } from "pinia";
import { ref, type Ref } from "vue";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useDocumentProcessor } from "../composables/useDocumentProcessor";
import { useNotificationStore } from "./notification";
import type { Attachment } from "../types/chat";
import {
  pickNativeAttachment,
  type AttachmentPickerMode,
} from "./attachmentNativePicker";
import { pickWebAttachment } from "./attachmentWebPicker";

function registerProgressListener(stagedAttachments: Ref<Attachment[]>) {
  void listen<any>("vcp-file-register-progress", (event) => {
    const { progress, stableId } = event.payload || {};
    if (!stableId) return;
    const index = stagedAttachments.value.findIndex(
      (attachment) => attachment.id === stableId,
    );
    if (index === -1 || stagedAttachments.value[index].status !== "loading")
      return;
    if (progress >= 99) {
      stagedAttachments.value[index].status = "processing";
      stagedAttachments.value[index].progress = undefined;
      return;
    }
    const currentProgress = stagedAttachments.value[index].progress || 0;
    if (progress > currentProgress) {
      stagedAttachments.value[index].progress = progress;
    }
  });
}

function resolveAttachmentAsset(attachment: Attachment) {
  const sourcePath = attachment.internalPath || attachment.src;
  if (
    attachment.status === "desktop_only" ||
    !attachment.type.startsWith("image/") ||
    !sourcePath ||
    sourcePath.startsWith("http") ||
    sourcePath.startsWith("data:")
  ) {
    return;
  }
  try {
    attachment.resolvedSrc = convertFileSrc(sourcePath);
  } catch (error) {
    console.warn(
      `[AttachmentStore] Failed to convert attachment image path ${attachment.name}:`,
      error,
    );
  }
}

function resolveMessageAssets(message: any) {
  if (!Array.isArray(message.attachments)) return;
  message.attachments.forEach((attachment: Attachment) => {
    resolveAttachmentAsset(attachment);
  });
}

async function preprocessDocuments(
  customList: Attachment[] | undefined,
  stagedAttachments: Ref<Attachment[]>,
) {
  const targetList = customList || stagedAttachments.value;
  if (targetList.length === 0) return;
  const processor = useDocumentProcessor();
  for (const attachment of targetList) {
    const extension = attachment.name.split(".").pop()?.toLowerCase();
    if (
      !["txt", "md", "csv", "json", "docx", "pdf"].includes(extension || "")
    ) {
      continue;
    }
    try {
      const result = await processor.processAttachment(attachment);
      if (result?.extractedText)
        attachment.extractedText = result.extractedText;
      if (result?.imageFrames) attachment.imageFrames = result.imageFrames;
    } catch (error) {
      console.error(
        `[AttachmentStore] JIT document processing failed for ${attachment.name}:`,
        error,
      );
    }
  }
}

function removeAttachment(stagedAttachments: Ref<Attachment[]>, index: number) {
  if (index < 0 || index >= stagedAttachments.value.length) return;
  const removed = stagedAttachments.value.splice(index, 1)[0];
  if (!removed?.hash) return;
  void invoke("cleanup_single_orphaned_attachment", {
    hash: removed.hash,
  }).catch((error) => {
    console.warn(
      `[AttachmentStore] Targeted GC failed for ${removed.name}:`,
      error,
    );
  });
}

function clearAttachments(
  stagedAttachments: Ref<Attachment[]>,
  performGc: boolean,
) {
  const toClear = [...stagedAttachments.value];
  stagedAttachments.value = [];
  if (!performGc) return;
  toClear.forEach((attachment) => {
    if (!attachment.hash) return;
    void invoke("cleanup_single_orphaned_attachment", {
      hash: attachment.hash,
    }).catch((error) => {
      console.warn(
        `[AttachmentStore] Targeted GC failed for ${attachment.name}:`,
        error,
      );
    });
  });
}

export const useAttachmentStore = defineStore("attachment", () => {
  const stagedAttachments = ref<Attachment[]>([]);
  registerProgressListener(stagedAttachments);
  const notificationStore = useNotificationStore();

  const handleAttachment = async (
    mode: AttachmentPickerMode = "file",
  ): Promise<void> => {
    const isAndroid = navigator.userAgent.toLowerCase().includes("android");
    if (isAndroid) {
      await pickNativeAttachment(
        { stagedAttachments, notificationStore },
        mode,
      );
      return;
    }
    await pickWebAttachment({ stagedAttachments, notificationStore }, mode);
  };

  return {
    stagedAttachments,
    handleAttachment,
    resolveMessageAssets,
    preProcessDocuments: (customList?: Attachment[]) =>
      preprocessDocuments(customList, stagedAttachments),
    removeStaged: (index: number) => removeAttachment(stagedAttachments, index),
    clearStaged: (performGc = false) =>
      clearAttachments(stagedAttachments, performGc),
  };
});
