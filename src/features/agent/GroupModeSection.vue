<script setup lang="ts">
import SettingsSection from "../../components/settings/SettingsSection.vue";

interface GroupModeConfig {
  mode: string;
  tagMatchMode: string;
}

const props = defineProps<{
  config: GroupModeConfig;
}>();

const modeOptions = [
  { value: "sequential", label: "顺序发言", desc: "成员按预定顺序轮流发言" },
  {
    value: "naturerandom",
    label: "自然随机",
    desc: "基于标签和 @提及智能选择发言者",
  },
  {
    value: "invite_only",
    label: "邀请发言",
    desc: "由用户手动点击或提示词邀约",
  },
];

const tagModeOptions = [
  { value: "strict", label: "严格模式" },
  { value: "natural", label: "自然模式" },
];
</script>

<template>
  <SettingsSection title="群聊模式" accent-color="bg-purple-500">
    <div class="card-modern space-y-4">
      <div>
        <label class="text-[10px] uppercase font-bold opacity-40 mb-3 block"
          >发言逻辑 (Mode)</label
        >
        <div class="grid grid-cols-1 gap-2">
          <button
            v-for="opt in modeOptions"
            :key="opt.value"
            class="flex flex-col p-3 rounded-xl border transition-all text-left"
            :class="
              props.config.mode === opt.value
                ? 'bg-blue-500/10 border-blue-500/30 text-blue-500'
                : 'bg-black/5 dark:bg-white/5 border-transparent opacity-60'
            "
            @click="props.config.mode = opt.value"
          >
            <span class="text-sm font-bold">{{ opt.label }}</span>
            <span class="text-[10px] mt-0.5 opacity-60">{{ opt.desc }}</span>
          </button>
        </div>
      </div>

      <div class="pt-4 border-t border-black/5 dark:border-white/5">
        <label class="text-[10px] uppercase font-bold opacity-40 mb-3 block"
          >Tag 匹配模式</label
        >
        <div class="flex gap-2">
          <button
            v-for="opt in tagModeOptions"
            :key="opt.value"
            class="flex-1 py-2.5 rounded-xl border transition-all text-[12px] font-bold"
            :class="
              props.config.tagMatchMode === opt.value
                ? 'bg-purple-500/10 border-purple-500/30 text-purple-500'
                : 'bg-black/5 dark:bg-white/5 border-transparent opacity-60'
            "
            @click="props.config.tagMatchMode = opt.value"
          >
            {{ opt.label }}
          </button>
        </div>
        <p class="mt-2 text-[9px] opacity-30 leading-tight">
          自然模式会区分 Tag 来源，尽量避免 Agent 因引用自身历史发言而重复触发。
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
