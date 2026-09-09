<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useChatHistoryStore } from "../../core/stores/chatHistoryStore";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useChatStreamStore } from "../../core/stores/chatStreamStore";
import { useAttachmentStore } from "../../core/stores/attachmentStore";
import { useNotificationStore } from "../../core/stores/notification";
import { canSendStagedAttachments } from "../../core/stores/attachmentSendGate";
import { useLongTextPaste } from "./composables/useLongTextPaste";
import { useInputEnhancerVoice } from "./composables/useInputEnhancerVoice";
import StagedAttachmentPreview from "./StagedAttachmentPreview.vue";
import AttachmentMenuPanel from "./components/AttachmentMenuPanel.vue";
import InputActionButtons from "./components/InputActionButtons.vue";
import GroupStopAllButton from "./components/GroupStopAllButton.vue";

const props = defineProps<{ disabled?: boolean }>();
const emit = defineEmits<{
  (event: "send", content: string): void;
  (event: "attach"): void;
  (event: "toggle-menu", visible: boolean): void;
  (event: "focus-input"): void;
}>();

const input = ref("");
const showAttachMenu = ref(false);
const historyStore = useChatHistoryStore();
const sessionStore = useChatSessionStore();
const streamStore = useChatStreamStore();
const attachmentStore = useAttachmentStore();
const notificationStore = useNotificationStore();
const textareaRef = ref<HTMLTextAreaElement | null>(null);
const rootRef = ref<HTMLElement | null>(null);

const isGenerating = computed(() => streamStore.activeStreamingIds.size > 0);
const hasContent = computed(
  () =>
    input.value.trim() !== "" || attachmentStore.stagedAttachments.length > 0,
);
const attachmentsBusy = computed(
  () => !canSendStagedAttachments(attachmentStore.stagedAttachments),
);
const sendDisabled = computed(
  () => Boolean(props.disabled) || attachmentsBusy.value,
);

const autoResize = () => {
  if (!textareaRef.value) return;
  textareaRef.value.style.height = "auto";
  textareaRef.value.style.height = `${textareaRef.value.scrollHeight}px`;
};

const handleSend = () => {
  if (sendDisabled.value || !hasContent.value) return;
  emit("send", input.value);
  input.value = "";
  showAttachMenu.value = false;
};

const {
  transcriptionResult,
  recordingDuration,
  isAudioMode,
  isSTTActive,
  isLongPressRecording,
  isSwipeCancel,
  handleSTTTouchStart,
  handleSTTTouchMove,
  handleSTTTouchEnd,
  handleSTTTouchCancel,
  handleIconTouchStart,
  handleIconTouchMove,
  handleIconTouchEnd,
  handleIconTouchCancel,
} = useInputEnhancerVoice({
  isDisabled: () => Boolean(props.disabled),
  input,
  textareaRef,
  autoResize,
  attachmentStore,
  notificationStore,
  send: handleSend,
});

const handleAction = () => {
  if (isGenerating.value) {
    Array.from(streamStore.activeStreamingIds).forEach((id) =>
      streamStore.stopMessage(id as string),
    );
    return;
  }
  handleSend();
};

const handleKeydown = (event: KeyboardEvent) => {
  if (event.key !== "Enter" || (!event.ctrlKey && !event.metaKey)) return;
  event.preventDefault();
  handleAction();
};

const handleFocus = () => {
  showAttachMenu.value = false;
  if (!props.disabled && historyStore.currentChatHistory.length > 0) {
    emit("focus-input");
  }
};

const triggerFilePick = async (mode: "camera" | "gallery" | "file") => {
  if (props.disabled) return;
  emit("attach");
  await attachmentStore.handleAttachment(mode);
};

const { handlePaste, handleBeforeInput } = useLongTextPaste(input);

watch(showAttachMenu, (visible) => emit("toggle-menu", visible));
watch(input, () => void nextTick(autoResize));
watch(
  () => historyStore.editMessageContent,
  async (newContent) => {
    if (!newContent) return;
    input.value = newContent;
    historyStore.editMessageContent = "";
    isAudioMode.value = false;
    await nextTick();
    textareaRef.value?.focus();
    textareaRef.value?.dispatchEvent(new Event("input", { bubbles: true }));
  },
);
watch(
  () => sessionStore.sharePrefillText,
  async (newText) => {
    if (!newText) return;
    input.value = newText;
    sessionStore.sharePrefillText = "";
    isAudioMode.value = false;
    await nextTick();
    textareaRef.value?.focus();
    textareaRef.value?.dispatchEvent(new Event("input", { bubbles: true }));
  },
);

const removeStagedAttachment = (index: number) =>
  attachmentStore.removeStaged(index);
const handleClickOutside = (event: MouseEvent) => {
  if (
    showAttachMenu.value &&
    rootRef.value &&
    !rootRef.value.contains(event.target as Node)
  ) {
    showAttachMenu.value = false;
  }
};

onMounted(() => document.addEventListener("click", handleClickOutside, true));
onUnmounted(() =>
  document.removeEventListener("click", handleClickOutside, true),
);
</script>

<template>
  <div
    ref="rootRef"
    class="px-1 py-1 w-full transition-opacity duration-300 no-swipe relative flex flex-col gap-1.5"
    :class="{ 'opacity-70 pointer-events-none': disabled }"
  >
    <GroupStopAllButton />

    <div
      v-if="attachmentStore.stagedAttachments.length > 0"
      class="flex items-center gap-2 mb-2 px-2 overflow-x-auto pb-1 pt-2"
    >
      <TransitionGroup name="list">
        <StagedAttachmentPreview
          v-for="(file, idx) in attachmentStore.stagedAttachments"
          :key="file.id || idx"
          :file="file"
          :index="idx"
          @remove="removeStagedAttachment"
        />
      </TransitionGroup>
    </div>

    <div class="flex items-end gap-2 px-1">
      <div
        class="flex-1 flex items-end gap-1.5 bg-[var(--secondary-bg)] border border-[var(--border-color)] rounded-2xl px-2 py-1 shadow-sm relative overflow-visible transition-all duration-300"
        :class="{
          'ring-1 ring-blue-500/30 border-blue-500/50':
            isSTTActive || isLongPressRecording,
          'ring-1 ring-red-500/30 border-red-500/50': isSwipeCancel,
        }"
      >
        <button
          :disabled="disabled"
          @touchstart.prevent="handleIconTouchStart"
          @touchmove="handleIconTouchMove"
          @touchend="handleIconTouchEnd"
          @touchcancel="handleIconTouchCancel"
          class="min-w-[44px] min-h-[44px] flex items-center justify-center shrink-0 rounded-full hover:bg-black/5 dark:hover:bg-white/5 text-[var(--primary-text)] opacity-90 active:scale-90 transition-all relative select-none touch-none disabled:opacity-40"
          :class="{
            'bg-blue-500/10 text-blue-500': isAudioMode || isLongPressRecording,
            'bg-red-500/10 text-red-500': isSwipeCancel && isLongPressRecording,
          }"
          aria-label="语音输入"
        >
          <svg
            v-if="isAudioMode"
            width="24"
            height="24"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2.2"
            stroke-linecap="round"
            stroke-linejoin="round"
            class="w-6 h-6 shrink-0 animate-fade-in"
          >
            <rect x="2" y="4" width="20" height="16" rx="2" ry="2"></rect>
            <line x1="6" y1="8" x2="6" y2="8"></line>
            <line x1="10" y1="8" x2="10" y2="8"></line>
            <line x1="14" y1="8" x2="14" y2="8"></line>
            <line x1="18" y1="8" x2="18" y2="8"></line>
            <line x1="6" y1="12" x2="6" y2="12"></line>
            <line x1="10" y1="12" x2="10" y2="12"></line>
            <line x1="14" y1="12" x2="14" y2="12"></line>
            <line x1="18" y1="12" x2="18" y2="12"></line>
            <line x1="7" y1="16" x2="17" y2="16"></line>
          </svg>
          <svg
            v-else
            width="26"
            height="26"
            viewBox="0 0 48 48"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
            class="shrink-0"
            :class="{ 'animate-pulse text-blue-500': isLongPressRecording }"
          >
            <circle
              cx="24"
              cy="24"
              r="19.5"
              stroke="currentColor"
              stroke-width="3.5"
              fill="none"
            />
            <circle cx="17.5" cy="24" r="3" fill="currentColor" />
            <path
              d="M 21.5 18 A 6.5 6.5 0 0 1 21.5 30"
              stroke="currentColor"
              stroke-width="3"
              stroke-linecap="round"
              fill="none"
            />
            <path
              d="M 26 13.5 A 12 12 0 0 1 26 34.5"
              stroke="currentColor"
              stroke-width="3"
              stroke-linecap="round"
              fill="none"
            />
          </svg>
        </button>

        <div
          class="flex-1 flex flex-col justify-end relative min-h-[36px] py-[1px] overflow-visible"
        >
          <textarea
            v-if="!isAudioMode && !isLongPressRecording"
            ref="textareaRef"
            v-model="input"
            @focus="handleFocus"
            @keydown="handleKeydown"
            @paste="handlePaste"
            @beforeinput="handleBeforeInput"
            rows="1"
            class="w-full bg-transparent border-none focus:outline-none focus:ring-0 text-[var(--primary-text)] text-[15px] placeholder-opacity-40 resize-none leading-[1.25] py-[8px] scrollbar-hide vcp-textarea"
            style="max-height: 114px"
            :placeholder="disabled ? '请先选择话题以开启对话' : '说点什么...'"
            :disabled="disabled"
          ></textarea>

          <div
            v-else-if="isAudioMode && !isSTTActive"
            @touchstart.prevent="handleSTTTouchStart"
            @touchmove="handleSTTTouchMove"
            @touchend="handleSTTTouchEnd"
            @touchcancel="handleSTTTouchCancel"
            class="w-full h-[36px] flex items-center justify-center rounded-xl bg-black/5 dark:bg-white/5 border border-black/10 dark:border-white/10 active:bg-black/15 dark:active:bg-white/15 select-none touch-none cursor-pointer transform active:scale-[0.98] transition-all duration-75"
          >
            <span
              class="text-[13px] font-semibold text-[var(--primary-text)] opacity-85 tracking-wider"
              >按住 说话</span
            >
          </div>

          <div
            v-else-if="isSTTActive"
            class="w-full flex flex-col justify-center min-h-[36px] py-1 px-1 select-none animate-fade-in"
          >
            <div
              class="flex items-center gap-1.5 text-xs font-semibold text-blue-500"
              :class="{ 'text-red-500': isSwipeCancel }"
            >
              <span
                class="w-2.5 h-2.5 rounded-full"
                :class="
                  isSwipeCancel
                    ? 'bg-red-500 animate-pulse'
                    : 'bg-blue-500 animate-ping'
                "
              ></span>
              <span>{{
                isSwipeCancel
                  ? "松手取消转写"
                  : "正在识别流式文字... (上滑取消)"
              }}</span>
            </div>
            <div
              class="text-[14px] text-[var(--primary-text)] min-h-[1.25rem] leading-[1.25] break-all opacity-85 italic font-medium mt-0.5"
            >
              {{ transcriptionResult || "请开始说话..." }}
            </div>
          </div>

          <div
            v-else-if="isLongPressRecording"
            class="w-full flex flex-col justify-center min-h-[36px] py-1 px-1 select-none animate-fade-in"
          >
            <div
              class="flex items-center gap-1.5 text-xs font-semibold text-blue-500"
              :class="{ 'text-red-500': isSwipeCancel }"
            >
              <span
                class="w-2.5 h-2.5 rounded-full"
                :class="
                  isSwipeCancel
                    ? 'bg-red-500 animate-pulse'
                    : 'bg-blue-500 animate-ping'
                "
              ></span>
              <span>{{
                isSwipeCancel ? "松手取消发送" : "倾听中... (上滑取消)"
              }}</span>
            </div>
            <div
              class="text-[14px] text-[var(--primary-text)] font-mono opacity-85 mt-0.5"
            >
              录音时长: {{ recordingDuration }} 秒
            </div>
          </div>

          <div
            class="absolute top-0 left-0 right-0 h-4 pointer-events-none bg-[var(--secondary-bg)] opacity-90"
            style="
              mask-image: linear-gradient(to bottom, black, transparent);
              -webkit-mask-image: linear-gradient(
                to bottom,
                black,
                transparent
              );
            "
          ></div>
        </div>

        <InputActionButtons
          :visible="showAttachMenu"
          :disabled="disabled"
          :has-content="hasContent"
          :is-generating="isGenerating"
          :send-disabled="sendDisabled"
          @toggle="showAttachMenu = $event"
          @action="handleAction"
        />
      </div>
    </div>

    <AttachmentMenuPanel
      :visible="showAttachMenu"
      :disabled="disabled"
      @pick="triggerFilePick"
    />
  </div>
</template>

<style src="./InputEnhancer.css" scoped></style>
