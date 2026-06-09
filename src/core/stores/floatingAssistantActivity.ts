import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

interface FloatingAssistantActivityPayload {
  activeCount?: number;
  isGenerating?: boolean;
}

export const useFloatingAssistantActivityStore = defineStore(
  "floatingAssistantActivity",
  () => {
    const activeCount = ref(0);
    const listening = ref(false);
    let unlisten: UnlistenFn | null = null;
    let listenPromise: Promise<void> | null = null;

    const isGenerating = computed(() => activeCount.value > 0);

    const applyPayload = (payload: FloatingAssistantActivityPayload) => {
      if (typeof payload.activeCount === "number") {
        activeCount.value = Math.max(0, payload.activeCount);
        return;
      }
      activeCount.value = payload.isGenerating ? 1 : 0;
    };

    const hydrate = async () => {
      try {
        const active = await invoke<boolean>("is_assistant_chat_active");
        activeCount.value = active ? Math.max(activeCount.value, 1) : 0;
      } catch (error) {
        console.error(
          "[FloatingAssistantActivity] Failed to hydrate assistant activity:",
          error,
        );
      }
    };

    const ensureListening = () => {
      if (listening.value || listenPromise) return;

      listenPromise = (async () => {
        await hydrate();
        unlisten = await listen<FloatingAssistantActivityPayload>(
          "floating-assistant-activity",
          (event) => applyPayload(event.payload || {}),
        );
        listening.value = true;
      })().catch((error) => {
        listenPromise = null;
        console.error(
          "[FloatingAssistantActivity] Failed to listen assistant activity:",
          error,
        );
      });
    };

    const dispose = () => {
      unlisten?.();
      unlisten = null;
      listening.value = false;
      listenPromise = null;
    };

    return {
      activeCount,
      isGenerating,
      listening,
      ensureListening,
      dispose,
    };
  },
);
