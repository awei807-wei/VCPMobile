<script setup lang="ts">
import VcpAvatar from "../../components/ui/VcpAvatar.vue";
import SettingsSection from "../../components/settings/SettingsSection.vue";
import type { Agent } from "./groupSettingsTypes";

const props = defineProps<{
  agents: Agent[];
  members: string[];
  memberTags: Record<string, string>;
}>();

const emit = defineEmits<{
  (event: "toggle-member", agentId: string): void;
}>();

const isMember = (agentId: string) => props.members.includes(agentId);
</script>

<template>
  <SettingsSection
    title="群组成员"
    description="勾选要加入群组的助手，并设置其触发标签"
  >
    <div class="card-modern !p-0 max-h-80 overflow-y-auto vcp-scrollable">
      <div
        v-for="agent in props.agents"
        :key="agent.id"
        class="flex items-center gap-3 p-3 border-b border-black/5 dark:border-white/5 last:border-0 active:bg-black/5 dark:active:bg-white/5 transition-all"
      >
        <input
          type="checkbox"
          :checked="isMember(agent.id)"
          class="w-5 h-5 rounded-md accent-blue-500 cursor-pointer"
          @change="emit('toggle-member', agent.id)"
        />

        <VcpAvatar
          owner-type="agent"
          :owner-id="agent.id"
          :fallback-name="agent.name"
          size="w-10 h-10"
          rounded="rounded-full"
          dominant-color="var(--primary)"
        />

        <div class="flex-1 min-w-0">
          <div class="text-sm font-bold truncate">{{ agent.name }}</div>
          <div v-if="isMember(agent.id)" class="mt-1">
            <input
              v-model="props.memberTags[agent.id]"
              placeholder="设置触发标签..."
              class="w-full bg-black/5 dark:bg-white/5 rounded-lg px-2 py-1.5 text-[11px] outline-none border border-transparent focus:border-blue-500/30 transition-all font-mono"
            />
          </div>
        </div>
      </div>
    </div>
  </SettingsSection>
</template>

<style scoped>
.card-modern {
  @apply bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 rounded-2xl p-4 shadow-sm;
}
</style>
