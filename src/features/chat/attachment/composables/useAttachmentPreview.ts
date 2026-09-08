import { computed, ref } from "vue";
import type { Attachment } from "../../../../core/types/chat";
import {
  canPreviewAttachment,
  formatFileSize,
  getAttachmentStats,
  getAttachmentType,
  getFileExtension,
  getPreviewComponentFor,
  getPreviewText as formatPreviewText,
  hasThumbnail,
  MAX_PREVIEW_TEXT_LENGTH,
} from "./attachmentPreviewSupport";

export { MAX_PREVIEW_TEXT_LENGTH };

/** 附件预览的响应式状态与操作。 */
export function useAttachmentPreview() {
  const isLoading = ref(false);
  const error = ref<string | null>(null);
  const previewCache = new Map<string, string>();
  const previewType = (attachment: Attachment) => getAttachmentType(attachment);
  const canPreview = computed(() => canPreviewAttachment);
  const getPreviewComponent = computed(() => getPreviewComponentFor);
  const getPreviewText = computed(() => formatPreviewText);
  const hasAttachmentThumbnail = computed(() => hasThumbnail);
  const getStats = computed(() => getAttachmentStats);
  const clearCache = (): void => {
    previewCache.clear();
    console.log("[useAttachmentPreview] 已清理预览缓存");
  };

  return {
    isLoading,
    error,
    previewCache,
    getAttachmentType: previewType,
    canPreview,
    getPreviewComponent,
    formatFileSize,
    getFileExtension,
    getPreviewText,
    hasThumbnail: hasAttachmentThumbnail,
    clearCache,
    getStats,
  };
}
