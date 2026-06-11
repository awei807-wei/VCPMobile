<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import SlidePage from "../../components/ui/SlidePage.vue";
import { useNotificationStore } from "../../core/stores/notification";
import { useSettingsStore } from "../../core/stores/settings";

const props = withDefaults(
  defineProps<{
    isOpen?: boolean;
    zIndex?: number;
  }>(),
  {
    isOpen: false,
    zIndex: 50,
  },
);

const emit = defineEmits<{
  close: [];
}>();

const settingsStore = useSettingsStore();
const notificationStore = useNotificationStore();

const AUTH_STORAGE_KEY = "vcp_fallback_auth";
const AUTH_USERNAME_STORAGE_KEY = "vcp_fallback_auth_username";
const DAILYNOTE_API_PATH = "/AdminPanel/dailynote_api";
const PREVIEW_PATH = "/dailynote-panel-preview/";

const iframeVersion = ref(0);
const frameReady = ref(false);
const shouldRenderFrame = ref(false);
const authStatus = ref("正在读取全局管理员鉴权…");
const frameRef = ref<HTMLIFrameElement | null>(null);
const currentAuthPayload = ref<{ username: string; token: string } | null>(null);
const currentDailyNoteApiBase = ref("");
const currentAppliedConfigKey = ref("");
let reloadRequestId = 0;
let suppressSettingsWatcher = false;

const dailyNoteConfigKey = computed(() =>
  JSON.stringify({
    profileId: settingsStore.settings?.activeConnectionProfileId || "",
    serverUrl: settingsStore.settings?.vcpServerUrl || "",
    username: settingsStore.settings?.adminUsername || "",
    password: settingsStore.settings?.adminPassword || "",
  }),
);

const frameSrc = computed(() => {
  const params = new URLSearchParams({
    embedded: "1",
    entry: "right-drawer",
    v: String(iframeVersion.value),
  });

  if (currentDailyNoteApiBase.value) {
    params.set("apiBase", currentDailyNoteApiBase.value);
  }

  return `${PREVIEW_PATH}?${params.toString()}`;
});

const utf8ToBase64 = (value: string) => {
  const bytes = new TextEncoder().encode(value);
  let binary = "";
  bytes.forEach((byte) => {
    binary += String.fromCharCode(byte);
  });
  return btoa(binary);
};

const buildDailyNoteApiBase = (serverUrl?: string | null) => {
  const rawUrl = serverUrl?.trim();
  if (!rawUrl) return "";

  try {
    const parsed = new URL(/^[a-z][a-z\d+\-.]*:\/\//i.test(rawUrl) ? rawUrl : `http://${rawUrl}`);
    const normalizedPath = parsed.pathname.replace(/\/+$/, "");
    const dailynoteIndex = normalizedPath
      .toLowerCase()
      .indexOf(DAILYNOTE_API_PATH.toLowerCase());

    parsed.pathname =
      dailynoteIndex >= 0
        ? normalizedPath.slice(0, dailynoteIndex) + DAILYNOTE_API_PATH
        : DAILYNOTE_API_PATH;
    parsed.username = "";
    parsed.password = "";
    parsed.search = "";
    parsed.hash = "";

    return parsed.toString().replace(/\/+$/, "");
  } catch (error) {
    console.warn("[DailyNoteView] Invalid VCP server URL for DailyNote API:", error);
    return "";
  }
};

const clearPreviewAuth = () => {
  localStorage.removeItem(AUTH_STORAGE_KEY);
  localStorage.removeItem(AUTH_USERNAME_STORAGE_KEY);
};

const injectGlobalAdminAuth = async () => {
  try {
    suppressSettingsWatcher = true;
    await settingsStore.fetchSettings();
    currentAppliedConfigKey.value = dailyNoteConfigKey.value;
  } catch (error: any) {
    clearPreviewAuth();
    authStatus.value = "读取全局配置失败，日记面板将停在鉴权提示页";
    notificationStore.addNotification({
      type: "error",
      title: "日记鉴权读取失败",
      message: error?.toString?.() || String(error),
      toastOnly: true,
    });
    currentAuthPayload.value = null;
    currentDailyNoteApiBase.value = "";
    return;
  } finally {
    suppressSettingsWatcher = false;
  }

  const apiBase = buildDailyNoteApiBase(settingsStore.settings?.vcpServerUrl);
  currentDailyNoteApiBase.value = apiBase;

  const username = settingsStore.settings?.adminUsername?.trim() || "";
  const password = settingsStore.settings?.adminPassword || "";

  if (!username || !password) {
    clearPreviewAuth();
    currentAuthPayload.value = null;
    authStatus.value = apiBase
      ? "全局配置未填写管理员账号密码，已同步 VCP 服务器地址"
      : "全局配置未填写管理员账号密码";
    return;
  }

  const token = utf8ToBase64(`${username}:${password}`);
  localStorage.setItem(AUTH_STORAGE_KEY, token);
  localStorage.setItem(AUTH_USERNAME_STORAGE_KEY, username);
  currentAuthPayload.value = { username, token };
  authStatus.value = apiBase
    ? "已复用全局管理员鉴权与 VCP 服务器地址"
    : "已复用全局管理员鉴权，日记 API 地址沿用面板配置";
};

const postConfigToFrame = () => {
  if (!frameRef.value?.contentWindow) return;

  frameRef.value.contentWindow.postMessage(
    {
      type: "VCP_DAILYNOTE_SET_AUTH",
      source: "VCPMobile",
      username: currentAuthPayload.value?.username || "",
      token: currentAuthPayload.value?.token || "",
      apiBase: currentDailyNoteApiBase.value,
    },
    window.location.origin,
  );
};

const reloadPanel = async () => {
  const requestId = ++reloadRequestId;
  shouldRenderFrame.value = false;
  frameReady.value = false;
  authStatus.value = "正在读取全局管理员鉴权…";
  await injectGlobalAdminAuth();
  if (!props.isOpen || requestId !== reloadRequestId) return;
  iframeVersion.value += 1;
  shouldRenderFrame.value = true;
};

const handleFrameLoad = () => {
  frameReady.value = true;
  // 面板内“清空鉴权”只清空预览本地 token；重新进入时仍以全局配置为准。
  void injectGlobalAdminAuth().then(postConfigToFrame);
};

const handleAuthStorageChange = (event: StorageEvent) => {
  if (!props.isOpen) return;
  if (event.key !== AUTH_STORAGE_KEY && event.key !== AUTH_USERNAME_STORAGE_KEY) return;

  // iframe 内手动清空或改写鉴权后，重新以 VCPMobile 全局管理员账号密码为准。
  void injectGlobalAdminAuth().then(postConfigToFrame);
};

watch(
  () => props.isOpen,
  (isOpen) => {
    if (isOpen) {
      void reloadPanel();
    } else {
      reloadRequestId += 1;
      shouldRenderFrame.value = false;
      frameReady.value = false;
    }
  },
  { immediate: true },
);

watch(
  dailyNoteConfigKey,
  (newKey, oldKey) => {
    if (!props.isOpen || suppressSettingsWatcher || !shouldRenderFrame.value) return;
    if (!oldKey || newKey === oldKey) return;
    if (newKey === currentAppliedConfigKey.value) return;

    authStatus.value = "检测到线路或管理员配置变化，正在刷新日记面板…";
    void reloadPanel();
  },
);

onMounted(() => {
  window.addEventListener("storage", handleAuthStorageChange);
});

onUnmounted(() => {
  reloadRequestId += 1;
  window.removeEventListener("storage", handleAuthStorageChange);
});
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <section class="daily-note-view pointer-events-auto flex h-full w-full flex-col bg-secondary-bg text-primary-text">
      <header class="daily-note-view__header">
        <div class="min-w-0">
          <div class="flex items-center gap-2">
            <h2 class="truncate text-xl font-black tracking-tight">日记流</h2>
            <span class="daily-note-view__badge">DailyNotePanel</span>
          </div>
          <p class="mt-1 truncate text-[10px] opacity-50">
            {{ authStatus }}
          </p>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          <button class="daily-note-view__ghost-button" type="button" @click="reloadPanel">
            刷新
          </button>
          <button
            class="daily-note-view__close-button"
            type="button"
            aria-label="关闭日记流"
            @click="emit('close')"
          >
            <svg width="22" height="22" viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <path d="M18 6 6 18M6 6l12 12" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" />
            </svg>
          </button>
        </div>
      </header>

      <div class="daily-note-view__frame-shell no-swipe">
        <div v-if="!frameReady" class="daily-note-view__loading">
          <span class="daily-note-view__spinner"></span>
          <span>正在打开日记面板…</span>
        </div>
        <iframe
          v-if="shouldRenderFrame"
          ref="frameRef"
          :key="iframeVersion"
          class="daily-note-view__frame"
          :class="{ 'is-ready': frameReady }"
          :src="frameSrc"
          title="日记流面板"
          @load="handleFrameLoad"
        ></iframe>
      </div>
    </section>
  </SlidePage>
</template>

<style scoped>
.daily-note-view__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: calc(var(--vcp-safe-top, 24px) + 12px) 16px 12px;
  border-bottom: 1px solid color-mix(in srgb, var(--primary-text) 8%, transparent);
  background:
    radial-gradient(circle at top left, color-mix(in srgb, var(--highlight-text) 16%, transparent), transparent 34%),
    color-mix(in srgb, var(--secondary-bg) 97%, transparent);
}

.daily-note-view__badge {
  border: 1px solid color-mix(in srgb, var(--highlight-text) 28%, transparent);
  border-radius: 999px;
  padding: 3px 8px;
  color: var(--highlight-text);
  background: color-mix(in srgb, var(--highlight-text) 10%, transparent);
  font-size: 9px;
  font-weight: 800;
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.daily-note-view__ghost-button,
.daily-note-view__close-button {
  min-width: 40px;
  height: 40px;
  border: 1px solid color-mix(in srgb, var(--primary-text) 8%, transparent);
  border-radius: 14px;
  background: color-mix(in srgb, var(--primary-text) 5%, transparent);
  color: var(--primary-text);
  transition: transform 0.18s ease, opacity 0.18s ease, background 0.18s ease;
}

.daily-note-view__ghost-button {
  padding: 0 12px;
  font-size: 12px;
  font-weight: 800;
}

.daily-note-view__close-button {
  display: flex;
  align-items: center;
  justify-content: center;
}

.daily-note-view__ghost-button:active,
.daily-note-view__close-button:active {
  transform: scale(0.94);
}

@media (hover: hover) {
  .daily-note-view__ghost-button:hover,
  .daily-note-view__close-button:hover {
    background: color-mix(in srgb, var(--highlight-text) 12%, transparent);
  }
}

.daily-note-view__frame-shell {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  background: #0f172a;
}

.daily-note-view__frame {
  width: 100%;
  height: 100%;
  border: 0;
  opacity: 0;
  background: #f7f7fb;
  transition: opacity 0.2s ease;
}

.daily-note-view__frame.is-ready {
  opacity: 1;
}

.daily-note-view__loading {
  position: absolute;
  inset: 0;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  color: rgba(255, 255, 255, 0.72);
  background: linear-gradient(135deg, #0f172a, #111827);
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0.04em;
}

.daily-note-view__spinner {
  width: 16px;
  height: 16px;
  border: 2px solid rgba(255, 255, 255, 0.24);
  border-top-color: rgba(255, 255, 255, 0.9);
  border-radius: 999px;
  animation: daily-note-spin 0.8s linear infinite;
}

@keyframes daily-note-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
