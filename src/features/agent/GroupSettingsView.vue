<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useAssistantStore } from "../../core/stores/assistant";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useNotificationStore } from "../../core/stores/notification";
import { useOverlayStore } from "../../core/stores/overlay";
import SlidePage from "../../components/ui/SlidePage.vue";
import AvatarCropper from "../../components/ui/AvatarCropper.vue";
import VcpAvatar from "../../components/ui/VcpAvatar.vue";
import GroupMembersSection from "./GroupMembersSection.vue";
import GroupModeSection from "./GroupModeSection.vue";
import GroupModelSection from "./GroupModelSection.vue";
import GroupPromptsSection from "./GroupPromptsSection.vue";
import { toggleGroupMember } from "./groupSettingsHelpers";
import type { Agent, GroupConfig } from "./groupSettingsTypes";

const props = withDefaults(
  defineProps<{
    id: string;
    isOpen?: boolean;
    zIndex?: number;
  }>(),
  {
    isOpen: false,
    zIndex: 50,
  },
);

const emit = defineEmits(["close"]);

const assistantStore = useAssistantStore();
const sessionStore = useChatSessionStore();
const notificationStore = useNotificationStore();
const overlayStore = useOverlayStore();

const groupConfig = ref<GroupConfig>({
  id: props.id,
  name: "",
  avatar: "",
  members: [],
  mode: "sequential",
  memberTags: {},
  groupPrompt: "",
  invitePrompt: "",
  useUnifiedModel: false,
  unifiedModel: "",
  tagMatchMode: "strict",
});

const allAgents = ref<Agent[]>([]);
const isSaving = ref(false);
const saveSuccess = ref(false);
let saveTimeout: ReturnType<typeof setTimeout> | null = null;
let saveSuccessTimer: ReturnType<typeof setTimeout> | null = null;

const originalConfig = ref<GroupConfig | null>(null);

onUnmounted(() => {
  if (saveTimeout) {
    clearTimeout(saveTimeout);
    saveTimeout = null;
  }
  if (saveSuccessTimer) {
    clearTimeout(saveSuccessTimer);
    saveSuccessTimer = null;
  }
});

const fileInput = ref<HTMLInputElement | null>(null);
const isCropping = ref(false);
const cropImg = ref("");
const avatarVersion = ref(0);

const triggerFileInput = () => {
  fileInput.value?.click();
};

const handleFileChange = (e: Event) => {
  const file = (e.target as HTMLInputElement).files?.[0];
  if (!file) return;

  const reader = new FileReader();
  reader.onload = (event) => {
    cropImg.value = event.target?.result as string;
    isCropping.value = true;
  };
  reader.readAsDataURL(file);
};

const onCropConfirm = async (blob: Blob) => {
  if (!groupConfig.value.id) return;

  isCropping.value = false;
  isSaving.value = true;

  try {
    const arrayBuffer = await blob.arrayBuffer();
    const bytes = new Uint8Array(arrayBuffer);

    // Use assistantStore to save avatar and get notification
    await assistantStore.saveAvatar(
      "group",
      groupConfig.value.id,
      blob.type,
      Array.from(bytes),
    );

    // Update UI local state via version
    avatarVersion.value = Date.now();
  } catch (err) {
    console.error("Failed to save avatar:", err);
  } finally {
    isSaving.value = false;
  }
};

const fetchAgents = () => {
  allAgents.value = assistantStore.agents.map((a) => ({
    id: a.id,
    name: a.name,
    avatar: "",
  }));
};

const fetchGroupConfig = async () => {
  if (!props.id) return;
  try {
    const config = await invoke<any>("read_group_config", {
      groupId: props.id,
    });
    const memberTags = config.memberTags || {};

    groupConfig.value = {
      ...config,
      memberTags: typeof memberTags === "object" ? memberTags : {},
    };
    originalConfig.value = JSON.parse(JSON.stringify(groupConfig.value));
  } catch (err) {
    console.error("Failed to load group config:", err);
  }
};

const autoSave = async () => {
  if (!groupConfig.value.id || !props.isOpen) return;

  isSaving.value = true;
  saveSuccess.value = false;

  try {
    // Use assistantStore to save group config and get notification
    await assistantStore.saveGroup(groupConfig.value);
    saveSuccess.value = true;
    // 保存成功后更新快照，避免重复保存相同内容
    originalConfig.value = JSON.parse(JSON.stringify(groupConfig.value));
    if (saveSuccessTimer) clearTimeout(saveSuccessTimer);
    saveSuccessTimer = setTimeout(() => {
      saveSuccess.value = false;
    }, 2000);
  } catch (err) {
    console.error("Auto save failed:", err);
  } finally {
    isSaving.value = false;
  }
};

watch(
  groupConfig,
  () => {
    if (!originalConfig.value || !props.isOpen) return;
    // 只有与原始快照不同时才触发保存，避免无意义的后端调用
    if (
      JSON.stringify(groupConfig.value) === JSON.stringify(originalConfig.value)
    ) {
      return;
    }
    if (saveTimeout) {
      clearTimeout(saveTimeout);
    }
    saveTimeout = setTimeout(() => {
      autoSave();
    }, 1000);
  },
  { deep: true },
);

watch(
  () => props.isOpen,
  (val) => {
    if (val) {
      fetchAgents();
      fetchGroupConfig();
    }
  },
);

const toggleMember = (agentId: string) => {
  toggleGroupMember(
    groupConfig.value.members,
    groupConfig.value.memberTags,
    agentId,
    allAgents.value,
  );
};

const handleDelete = async () => {
  const confirmed = await overlayStore.showConfirm({
    title: "删除群组",
    message: `确定要删除群组“${groupConfig.value.name || props.id}”吗？相关聊天记录也会被标记为删除。`,
    confirmText: "删除",
    isDanger: true,
  });
  if (!confirmed) return;

  try {
    await assistantStore.deleteGroup(props.id);
    if (
      sessionStore.currentSelectedItem?.type === "group" &&
      sessionStore.currentSelectedItem?.id === props.id
    ) {
      sessionStore.currentSelectedItem = null;
    }
    emit("close");
  } catch (err) {
    console.error("Failed to delete group:", err);
    notificationStore.addNotification({
      type: "error",
      title: "删除群组失败",
      message: "群组未被删除，请稍后重试。",
      toastOnly: true,
    });
  }
};

onMounted(async () => {
  if (props.isOpen) {
    fetchAgents();
    await fetchGroupConfig();
  }
});
</script>

<template>
  <SlidePage :is-open="props.isOpen" :z-index="props.zIndex">
    <div
      class="group-settings-view flex flex-col h-full w-full bg-secondary-bg text-primary-text pointer-events-auto"
    >
      <!-- Header -->
      <header
        class="p-3 flex items-center justify-between border-b border-black/10 dark:border-white/10 pt-[calc(var(--vcp-safe-top,24px)+10px)] pb-3 shrink-0 bg-black/5 dark:bg-white/5"
      >
        <div class="flex items-center gap-2">
          <button
            @click="emit('close')"
            class="p-2 hover:bg-black/5 dark:hover:bg-white/10 rounded-lg active:scale-95 transition-all"
          >
            <svg
              width="20"
              height="20"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <line x1="19" y1="12" x2="5" y2="12"></line>
              <polyline points="12 19 5 12 12 5"></polyline>
            </svg>
          </button>
          <h2 class="text-base font-bold">群组设置</h2>
        </div>
        <div
          class="text-xs font-bold transition-opacity duration-300"
          :class="{
            'opacity-100': isSaving || saveSuccess,
            'opacity-0': !isSaving && !saveSuccess,
          }"
        >
          <span v-if="isSaving" class="text-blue-400 animate-pulse"
            >保存中...</span
          >
          <span v-else-if="saveSuccess" class="text-green-500">已保存 ✅</span>
        </div>
      </header>

      <div class="flex-1 overflow-y-auto p-5 space-y-8 pb-safe no-rubber-band">
        <!-- 1. Basic Info -->
        <section class="flex flex-col items-center gap-6 py-2">
          <div class="relative group" @click="triggerFileInput">
            <VcpAvatar
              owner-type="group"
              :owner-id="props.id"
              :version="avatarVersion"
              :fallback-name="groupConfig.name"
              size="w-24 h-24"
              rounded="rounded-full"
              :dominant-color="groupConfig.avatarCalculatedColor"
              class="border-2 border-dashed border-black/10 dark:border-white/20 shadow-inner group-active:scale-95 transition-all"
            />
            <div
              class="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 rounded-full flex items-center justify-center transition-opacity cursor-pointer z-20"
            >
              <span
                class="text-[10px] text-white font-bold tracking-widest uppercase"
                >更换头像</span
              >
            </div>
            <input
              type="file"
              ref="fileInput"
              class="hidden"
              accept="image/*"
              @change="handleFileChange"
            />
          </div>

          <div class="w-full">
            <label
              class="text-[11px] uppercase font-black tracking-widest opacity-40 mb-2 block text-center"
              >群组名称</label
            >
            <input
              v-model="groupConfig.name"
              placeholder="设置群组名称..."
              class="bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 w-full rounded-2xl focus:border-blue-500/50 outline-none py-3.5 px-4 text-center text-lg font-bold transition-all text-primary-text shadow-sm"
            />
          </div>
        </section>

        <!-- 2. Members Section -->
        <GroupMembersSection
          :agents="allAgents"
          :members="groupConfig.members"
          :member-tags="groupConfig.memberTags"
          @toggle-member="toggleMember"
        />

        <!-- 3. Chat Modes -->
        <GroupModeSection :config="groupConfig" />

        <!-- 4. Model Settings -->
        <GroupModelSection :config="groupConfig" />

        <!-- 5. Prompts -->
        <GroupPromptsSection :config="groupConfig" />

        <!-- Actions -->
        <div class="pt-4 pb-8">
          <button
            @click="handleDelete"
            class="w-full py-3.5 bg-transparent border border-red-500/20 text-red-500/60 hover:bg-red-500/5 active:bg-red-500/10 active:scale-95 transition-all rounded-2xl font-black uppercase tracking-widest text-[11px]"
          >
            删除此群组
          </button>
        </div>
      </div>
    </div>
  </SlidePage>
  <!-- 头像裁剪器 -->
  <AvatarCropper
    v-if="isCropping"
    :img="cropImg"
    @cancel="isCropping = false"
    @confirm="onCropConfirm"
  />
</template>

<style scoped src="./GroupSettingsView.css"></style>
