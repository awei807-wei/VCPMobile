import { ref, watch, type Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { useAssistantStore } from "../stores/assistant";
import type { useAppLifecycleStore } from "../stores/appLifecycle";
import type { useChatSessionStore } from "../stores/chatSessionStore";
import type {
  PickedFileInfo,
  SharedContentData,
  SharedFileEntry,
} from "./appTypes";

type LifecycleStore = ReturnType<typeof useAppLifecycleStore>;
type AssistantStore = ReturnType<typeof useAssistantStore>;
type SessionStore = ReturnType<typeof useChatSessionStore>;

export interface ShareIntentOptions {
  lifecycleStore: LifecycleStore;
  assistantStore: AssistantStore;
  sessionStore: SessionStore;
}

interface ShareIntentState {
  sharedContent: Ref<SharedContentData>;
  showShareSelector: Ref<boolean>;
  pendingSharedFiles: Ref<PickedFileInfo[]>;
}

const createShareIntentState = (): ShareIntentState => ({
  sharedContent: ref({ text: "", files: [] }),
  showShareSelector: ref(false),
  pendingSharedFiles: ref([]),
});

const registerSharedFiles = async (
  files: SharedFileEntry[],
): Promise<PickedFileInfo[]> => {
  try {
    console.log(`[App] Registering ${files.length} shared file(s)...`);
    const results = await invoke<PickedFileInfo[]>(
      "plugin:vcp-mobile|register_shared_files",
      {
        files: files.map((file) => ({
          cachePath: file.cachePath,
          mimeType: file.mimeType,
          fileName: file.fileName,
        })),
      },
    );
    console.log("[App] Shared files registered:", results);
    return results;
  } catch (error) {
    console.error("[App] Failed to register shared files:", error);
    return [];
  }
};

const ensureAgentsLoaded = async (assistantStore: AssistantStore) => {
  if (assistantStore.agents.length > 0) return;
  try {
    await assistantStore.fetchAgents();
  } catch (error) {
    console.error("[App] Failed to fetch agents for share selector:", error);
  }
};

const prepareShareFiles = async (
  state: ShareIntentState,
  assistantStore: AssistantStore,
) => {
  const files = state.sharedContent.value.files;
  state.pendingSharedFiles.value =
    files.length > 0 ? await registerSharedFiles(files) : [];
  await ensureAgentsLoaded(assistantStore);
  state.showShareSelector.value = true;
};

const processSharedIntent = async (
  state: ShareIntentState,
  options: ShareIntentOptions,
  detail: any,
) => {
  console.log("[App] Share intent received:", detail);
  const text = typeof detail?.text === "string" ? detail.text : "";
  const files: SharedFileEntry[] = Array.isArray(detail?.files)
    ? detail.files
    : [];
  state.sharedContent.value = { text, files };
  if (options.lifecycleStore.state !== "READY") {
    console.log(
      "[App] Core not ready yet, deferring share intent processing...",
    );
    const unwatch = watch(
      () => options.lifecycleStore.state,
      async (currentState) => {
        if (currentState !== "READY") return;
        unwatch();
        await prepareShareFiles(state, options.assistantStore);
      },
    );
    return;
  }
  await prepareShareFiles(state, options.assistantStore);
};

const handleShareAgentSelected = async (
  state: ShareIntentState,
  sessionStore: SessionStore,
  agent: any,
) => {
  state.showShareSelector.value = false;
  try {
    await sessionStore.startShareSession(
      agent.id,
      state.sharedContent.value.text,
      state.pendingSharedFiles.value,
    );
  } catch (error) {
    console.error("[App] Failed to start share session:", error);
  }
  state.sharedContent.value = { text: "", files: [] };
  state.pendingSharedFiles.value = [];
};

export function useAppShareIntent(options: ShareIntentOptions) {
  const state = createShareIntentState();
  return {
    sharedContent: state.sharedContent,
    showShareSelector: state.showShareSelector,
    pendingSharedFiles: state.pendingSharedFiles,
    handleShareIntent: (event: Event) =>
      void processSharedIntent(state, options, (event as CustomEvent).detail),
    handleShareAgentSelected: (agent: any) =>
      handleShareAgentSelected(state, options.sessionStore, agent),
    handleShareSelectorClose: () => {
      state.showShareSelector.value = false;
    },
  };
}
