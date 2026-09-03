<script setup lang="ts">
import { ref } from "vue";
import ModelSelector from "../../components/ModelSelector.vue";
import SettingsRow from "../../components/settings/SettingsRow.vue";
import SettingsSection from "../../components/settings/SettingsSection.vue";
import SettingsSwitch from "../../components/settings/SettingsSwitch.vue";

interface GroupModelConfig {
  useUnifiedModel: boolean;
  unifiedModel?: string;
}

const props = defineProps<{
  config: GroupModelConfig;
}>();

const showModelSelector = ref(false);
const onModelSelect = (modelId: string) => {
  props.config.unifiedModel = modelId;
};
</script>

<template>
  <SettingsSection title="模型设置" accent-color="bg-orange-500">
    <div class="card-modern">
      <SettingsRow
        title="启用群组统一模型"
        description="所有成员强制使用同一模型，忽略其各自配置"
      >
        <template #action>
          <SettingsSwitch v-model="props.config.useUnifiedModel" />
        </template>
      </SettingsRow>

      <div
        v-if="props.config.useUnifiedModel"
        class="mt-4 pt-4 border-t border-black/5 dark:border-white/5"
      >
        <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block"
          >选择群组统一模型</label
        >
        <div class="flex gap-2">
          <input
            v-model="props.config.unifiedModel"
            readonly
            class="flex-1 bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono cursor-pointer"
            @click="showModelSelector = true"
          />
          <button
            class="w-12 h-12 bg-orange-500/10 text-orange-500 rounded-xl flex items-center justify-center active:scale-90 transition-all"
            @click="showModelSelector = true"
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
              <path
                d="M8.25 15L12 18.75 15.75 15m-7.5-6L12 5.25 15.75 9"
              ></path>
            </svg>
          </button>
        </div>
      </div>
    </div>
  </SettingsSection>

  <ModelSelector
    v-model="showModelSelector"
    :current-model="props.config.unifiedModel"
    title="选择群组统一模型"
    @select="onModelSelect"
  />
</template>

<style scoped>
.card-modern {
  @apply bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 rounded-2xl p-4 shadow-sm;
}
</style>
