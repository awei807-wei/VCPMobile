import { nextTick, ref, type Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useSpeechRecognition } from "../../../core/composables/useSpeechRecognition";
import { useAudioRecorder } from "../../../core/composables/useAudioRecorder";
import type { Attachment } from "../../../core/types/chat";

interface NotificationSink {
  addNotification(notification: {
    type: "warning" | "error";
    title: string;
    message: string;
    toastOnly: boolean;
  }): void;
}

interface AttachmentSink {
  stagedAttachments: Attachment[];
}

interface VoiceControllerOptions {
  isDisabled: () => boolean;
  input: Ref<string>;
  textareaRef: Ref<HTMLTextAreaElement | null>;
  autoResize: () => void;
  attachmentStore: AttachmentSink;
  notificationStore: NotificationSink;
  send: () => void;
}

export function useInputEnhancerVoice(options: VoiceControllerOptions) {
  const {
    isDisabled,
    input,
    textareaRef,
    autoResize,
    attachmentStore,
    notificationStore,
    send,
  } = options;
  const {
    transcriptionResult,
    startListening,
    stopListening,
    cancelListening,
  } = useSpeechRecognition();
  const { recordingDuration, startRecording, stopRecording, cancelRecording } =
    useAudioRecorder();

  const isAudioMode = ref(false);
  const isSTTActive = ref(false);
  const isLongPressRecording = ref(false);
  const isSwipeCancel = ref(false);
  let touchStartY = 0;
  let iconTouchStartTime = 0;
  let isIconLongPress = false;
  let iconLongPressTimeout: number | null = null;

  const notify = (title: string, message: string) => {
    notificationStore.addNotification({
      type: "warning",
      title,
      message,
      toastOnly: true,
    });
  };

  const toggleAudioMode = () => {
    if (isDisabled()) return;
    if (isSTTActive.value) {
      cancelListening();
      isSTTActive.value = false;
    }
    if (isLongPressRecording.value) {
      cancelRecording();
      isLongPressRecording.value = false;
    }
    isAudioMode.value = !isAudioMode.value;
    isSwipeCancel.value = false;
    if (navigator.vibrate) navigator.vibrate(35);
    if (!isAudioMode.value) {
      void nextTick(() => {
        textareaRef.value?.focus();
        autoResize();
      });
    }
  };

  const handleSTTTouchStart = async (event: TouchEvent) => {
    if (isDisabled()) return;
    if (event.cancelable) event.preventDefault();
    isSTTActive.value = true;
    isSwipeCancel.value = false;
    touchStartY = event.touches[0].clientY;
    try {
      await startListening(() => undefined);
      if (navigator.vibrate) navigator.vibrate(50);
    } catch (error: any) {
      isSTTActive.value = false;
      notify("语音转写启动失败", error?.message || String(error));
    }
  };

  const updateSwipeCancel = (event: TouchEvent) => {
    const diffY = event.touches[0].clientY - touchStartY;
    const shouldCancel = diffY < -60;
    if (shouldCancel === isSwipeCancel.value) return;
    isSwipeCancel.value = shouldCancel;
    if (navigator.vibrate) navigator.vibrate(30);
  };

  const handleSTTTouchMove = (event: TouchEvent) => {
    if (isSTTActive.value) updateSwipeCancel(event);
  };

  const handleSTTTouchEnd = async (event: TouchEvent) => {
    if (event.cancelable) event.preventDefault();
    if (!isSTTActive.value) return;
    isSTTActive.value = false;
    if (isSwipeCancel.value) {
      cancelListening();
      notify("已取消转写", "上滑取消操作已完成");
    } else {
      const recognizedText = await stopListening();
      if (recognizedText && !recognizedText.startsWith("[")) {
        input.value += recognizedText;
        isAudioMode.value = false;
        await nextTick();
        textareaRef.value?.focus();
        autoResize();
        if (navigator.vibrate) navigator.vibrate([40, 40]);
      }
    }
    isSwipeCancel.value = false;
  };

  const handleSTTTouchCancel = (event: TouchEvent) => {
    if (event.cancelable) event.preventDefault();
    if (!isSTTActive.value) return;
    cancelListening();
    isSTTActive.value = false;
    isSwipeCancel.value = false;
  };

  const handleIconTouchStart = (event: TouchEvent) => {
    if (isDisabled()) return;
    if (isAudioMode.value) {
      if (event.cancelable) event.preventDefault();
      toggleAudioMode();
      return;
    }
    if (event.cancelable) event.preventDefault();
    iconTouchStartTime = Date.now();
    isIconLongPress = false;
    isSwipeCancel.value = false;
    touchStartY = event.touches[0].clientY;
    iconLongPressTimeout = window.setTimeout(async () => {
      isIconLongPress = true;
      isLongPressRecording.value = true;
      try {
        await startRecording();
        if (navigator.vibrate) navigator.vibrate(50);
      } catch {
        isLongPressRecording.value = false;
        isIconLongPress = false;
      }
    }, 350);
  };

  const handleIconTouchMove = (event: TouchEvent) => {
    if (isIconLongPress && isLongPressRecording.value) updateSwipeCancel(event);
  };

  const sendRecording = async (result: { bytes: Uint8Array; blob: Blob }) => {
    try {
      const finalData = await invoke<any>("store_file", {
        originalName: `Voice_${Date.now()}.webm`,
        fileBytes: result.bytes,
        mimeType: result.blob.type || "audio/webm",
      });
      if (!finalData) throw new Error("语音附件保存失败");
      attachmentStore.stagedAttachments.unshift({
        id: `att_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`,
        type: finalData.type,
        src: finalData.internalPath,
        name: finalData.name,
        size: finalData.size,
        hash: finalData.hash,
        status: "done",
      });
      if (navigator.vibrate) navigator.vibrate([40, 40]);
      await nextTick();
      send();
    } catch (error: any) {
      console.error("[输入增强器] 直接发送语音失败:", error);
      notify("语音发送异常", String(error));
    }
  };

  const finishRecording = async () => {
    isLongPressRecording.value = false;
    if (isSwipeCancel.value) {
      cancelRecording();
      notify("已取消录音", "上滑取消操作已完成");
      return;
    }
    const result = await stopRecording();
    if (result) await sendRecording(result);
  };

  const handleIconTouchEnd = async (event: TouchEvent) => {
    if (event.cancelable) event.preventDefault();
    if (iconLongPressTimeout) {
      clearTimeout(iconLongPressTimeout);
      iconLongPressTimeout = null;
    }
    const duration = Date.now() - iconTouchStartTime;
    if (!isIconLongPress && duration < 350) {
      toggleAudioMode();
    } else if (isLongPressRecording.value) {
      await finishRecording();
    }
    isIconLongPress = false;
    isSwipeCancel.value = false;
  };

  const handleIconTouchCancel = (event: TouchEvent) => {
    if (event.cancelable) event.preventDefault();
    if (iconLongPressTimeout) {
      clearTimeout(iconLongPressTimeout);
      iconLongPressTimeout = null;
    }
    if (isLongPressRecording.value) cancelRecording();
    isLongPressRecording.value = false;
    isSwipeCancel.value = false;
    isIconLongPress = false;
  };

  return {
    transcriptionResult,
    recordingDuration,
    isAudioMode,
    isSTTActive,
    isLongPressRecording,
    isSwipeCancel,
    toggleAudioMode,
    handleSTTTouchStart,
    handleSTTTouchMove,
    handleSTTTouchEnd,
    handleSTTTouchCancel,
    handleIconTouchStart,
    handleIconTouchMove,
    handleIconTouchEnd,
    handleIconTouchCancel,
  };
}
