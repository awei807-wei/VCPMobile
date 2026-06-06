<script setup lang="ts">
// ToolInteractionOverlay.vue
// Container for Interactive tool UIs (camera, biometric, etc.)
// Phase 3 skeleton — will be populated when Interactive tools are added.

import { ref, onMounted, onUnmounted } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

interface ToolUiRequest {
  tool: string;
  id: string;
  args?: Record<string, any>;
}

interface DistributedNotificationPayload {
  title?: unknown;
  body?: unknown;
  androidNotification?: {
    attempted?: boolean;
    delivered?: boolean;
    error?: string | null;
    [key: string]: unknown;
  };
  [key: string]: unknown;
}

const activeRequest = ref<ToolUiRequest | null>(null);
let unlisten: UnlistenFn | null = null;

const toNotificationText = (value: unknown, fallback = ""): string => {
  if (typeof value === "string") return value;
  if (value === null || value === undefined) return fallback;
  return String(value);
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);

const normalizeDistributedNotification = (payload: unknown) => {
  const record = isRecord(payload) ? payload : {};
  const androidNotification = isRecord(record.androidNotification)
    ? record.androidNotification
    : {};
  const title = toNotificationText(record.title, "VCP Notification").trim() || "VCP Notification";
  const body = toNotificationText(record.body);

  return {
    title,
    body,
    androidAttempted: androidNotification.attempted === true,
    androidSkipped: androidNotification.attempted === false,
    delivered: androidNotification.delivered === true,
  };
};

const showBrowserNotificationFallback = (title: string, body: string) => {
  if ("Notification" in window && Notification.permission === "granted") {
    new Notification(title, { body });
  }
};

onMounted(async () => {
  // Listen for tool UI requests from the Rust backend
  unlisten = await listen<ToolUiRequest>("tool-ui-request", (event) => {
    activeRequest.value = event.payload;
  });
});

onUnmounted(() => {
  unlisten?.();
});

// Notification handler — listens for distributed-notification events
let unlistenNotification: UnlistenFn | null = null;

onMounted(async () => {
  unlistenNotification = await listen<DistributedNotificationPayload>(
    "distributed-notification",
    async (event) => {
      const notification = normalizeDistributedNotification(event.payload);
      const canRetryNative = !notification.delivered && !notification.androidSkipped;
      if (canRetryNative) {
        try {
          await invoke("plugin:vcp-mobile|show_system_notification", {
            title: notification.title,
            body: notification.body,
          });
        } catch (error) {
          console.warn("[Distributed Notification] Native notification failed:", error);
          showBrowserNotificationFallback(notification.title, notification.body);
        }
      } else if (!notification.delivered) {
        showBrowserNotificationFallback(notification.title, notification.body);
      }
      console.log(
        `[Distributed Notification] delivered=${notification.delivered} androidAttempted=${notification.androidAttempted} titleLength=${notification.title.length} bodyLength=${notification.body.length}`,
      );
    },
  );
});

onUnmounted(() => {
  unlistenNotification?.();
});

// Clipboard write handler
let unlistenClipboard: UnlistenFn | null = null;

onMounted(async () => {
  unlistenClipboard = await listen<{ content: string }>(
    "distributed-clipboard-write",
    async (event) => {
      try {
        await navigator.clipboard.writeText(event.payload.content);
        console.log("[Distributed Clipboard] Content written.");
      } catch (e) {
        console.error("[Distributed Clipboard] Write failed:", e);
      }
    },
  );
});

onUnmounted(() => {
  unlistenClipboard?.();
});
</script>

<template>
  <!-- Interactive tool UI overlay — shown when a tool needs user interaction -->
  <Teleport to="#vcp-feature-overlays">
    <Transition name="fade">
      <div
        v-if="activeRequest"
        class="fixed inset-0 z-overlay flex items-center justify-center bg-black/60"
      >
        <div
          class="bg-[var(--secondary-bg)] rounded-2xl p-6 mx-4 max-w-sm w-full shadow-2xl"
        >
          <div class="text-center space-y-3">
            <div class="text-lg font-bold text-primary-text">
              工具请求: {{ activeRequest.tool }}
            </div>
            <div class="text-sm opacity-60">
              此工具需要您的操作才能完成。
            </div>
            <!-- Phase 3+: tool-specific UI components will be rendered here -->
            <!-- e.g. <CameraCapture v-if="activeRequest.tool === 'camera'" /> -->
            <!-- e.g. <BiometricPrompt v-if="activeRequest.tool === 'biometric'" /> -->
            <div class="pt-4">
              <button
                class="px-4 py-2 bg-white/10 rounded-lg text-sm active:scale-95 transition-transform"
                @click="activeRequest = null"
              >
                取消
              </button>
            </div>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>
