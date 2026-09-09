<script setup lang="ts">
import { ref } from "vue";
import ModelSelector from "../../components/ModelSelector.vue";
import type { AgentConfig } from "./agentSettingsTypes";

const props = defineProps<{
  config: AgentConfig;
}>();

const sections = ref({ params: false });
const showModelSelector = ref(false);

const toggleSection = () => {
  sections.value.params = !sections.value.params;
};

const onModelSelect = (modelId: string) => {
  props.config.model = modelId;
};
</script>

<template>
  <section class="space-y-3">
    <button
      class="w-full flex items-center justify-between px-2 py-1"
      @click="toggleSection"
    >
      <div class="flex items-center gap-2">
        <div class="w-1 h-4 bg-blue-500 rounded-full"></div>
        <h3 class="text-xs font-black uppercase tracking-[0.2em] opacity-50">
          模型参数配置
        </h3>
      </div>
      <svg
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="3"
        stroke-linecap="round"
        stroke-linejoin="round"
        class="transition-transform duration-300"
        :class="{ 'rotate-180': sections.params }"
      >
        <polyline points="6 9 12 15 18 9"></polyline>
      </svg>
    </button>

    <div
      v-if="sections.params"
      class="card-modern space-y-5 animate-in fade-in slide-in-from-top-2 duration-300"
    >
      <div>
        <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block"
          >模型名称</label
        >
        <div class="flex gap-2">
          <input
            v-model="props.config.model"
            class="flex-1 bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none focus:bg-black/10 transition-all font-mono"
          />
          <button
            class="w-12 h-12 bg-blue-500/10 text-blue-500 rounded-xl flex-center active:scale-90 transition-all"
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

      <div
        :class="{
          'opacity-30 pointer-events-none': !props.config.useTemperature,
        }"
        class="transition-opacity duration-200"
      >
        <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block"
          >Temperature (0-2):</label
        >
        <input
          v-model.number="props.config.temperature"
          type="number"
          min="0"
          max="2"
          step="0.1"
          :disabled="!props.config.useTemperature"
          class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono"
        />
      </div>

      <div class="grid grid-cols-2 gap-5">
        <div>
          <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block"
            >上下文 Token 上限</label
          >
          <input
            v-model.number="props.config.contextTokenLimit"
            type="number"
            class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono"
          />
        </div>
        <div>
          <label class="text-[10px] uppercase font-bold opacity-40 mb-2 block"
            >最大输出 Token</label
          >
          <input
            v-model.number="props.config.maxOutputTokens"
            type="number"
            class="w-full bg-black/5 dark:bg-white/5 rounded-xl px-4 py-3 text-sm outline-none font-mono"
          />
        </div>
      </div>

      <div class="flex justify-between items-center py-2">
        <span class="text-sm font-medium">流式输出</span>
        <label class="relative inline-flex items-center cursor-pointer">
          <input
            v-model="props.config.streamOutput"
            type="checkbox"
            class="sr-only peer"
          />
          <div
            class="w-10 h-5 bg-black/10 dark:bg-white/10 rounded-full peer peer-checked:bg-blue-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-5"
          ></div>
        </label>
      </div>

      <div class="flex justify-between items-center py-2">
        <span class="text-sm font-medium">发送温度参数</span>
        <label class="relative inline-flex items-center cursor-pointer">
          <input
            v-model="props.config.useTemperature"
            type="checkbox"
            class="sr-only peer"
          />
          <div
            class="w-10 h-5 bg-black/10 dark:bg-white/10 rounded-full peer peer-checked:bg-blue-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-5"
          ></div>
        </label>
      </div>
    </div>
  </section>

  <ModelSelector
    v-model="showModelSelector"
    :current-model="props.config.model"
    title="选择助手模型"
    @select="onModelSelect"
  />
</template>

<style scoped>
.card-modern {
  @apply bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/10 rounded-xl p-4 shadow-sm;
}

.flex-center {
  @apply flex items-center justify-center;
}
</style>
