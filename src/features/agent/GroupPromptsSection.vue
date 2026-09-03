<script setup lang="ts">
import SettingsSection from "../../components/settings/SettingsSection.vue";

interface GroupPromptsConfig {
  groupPrompt: string;
  invitePrompt: string;
}

const props = defineProps<{
  config: GroupPromptsConfig;
}>();
</script>

<template>
  <SettingsSection title="提示词设置" accent-color="bg-green-500">
    <div class="space-y-4">
      <div class="card-modern">
        <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block"
          >群组提示词 (Group Prompt)</label
        >
        <textarea
          v-model="props.config.groupPrompt"
          placeholder="在这里输入群组的全局背景信息..."
          class="w-full bg-black/5 dark:bg-white/5 rounded-xl p-3 text-sm outline-none min-h-[100px] resize-none focus:bg-black/10 transition-all leading-relaxed"
        ></textarea>
      </div>

      <div class="card-modern">
        <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block"
          >邀请提示词 (Invite Prompt)</label
        >
        <textarea
          v-model="props.config.invitePrompt"
          placeholder="当轮到某位助手发言时的提示语..."
          class="w-full bg-black/5 dark:bg-white/5 rounded-xl p-3 text-xs outline-none min-h-[80px] resize-none focus:bg-black/10 transition-all leading-relaxed font-mono"
        ></textarea>
        <p v-pre class="mt-2 text-[9px] opacity-30">
          使用 {{ VCPChatAgentName }} 作为占位符。
        </p>
      </div>
    </div>
  </SettingsSection>
</template>

<style scoped>
.card-modern {
  @apply bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 rounded-2xl p-4 shadow-sm;
}
</style>
