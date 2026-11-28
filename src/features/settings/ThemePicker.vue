<script setup lang="ts">
import { onMounted, computed } from "vue";
import { useThemeStore, type ThemeInfo } from "../../core/stores/theme";

// 为 layout 提供默认值，防止由于未传参导致的初始化失败
const props = withDefaults(
  defineProps<{
    layout?: "horizontal" | "grid";
  }>(),
  {
    layout: "grid",
  },
);

const themeStore = useThemeStore();
const thumbnails = computed(() => themeStore.themeThumbnails);

onMounted(async () => {
  try {
    await themeStore.fetchThemes();
  } catch (e) {
    console.error("[ThemePicker] Initialization failed:", e);
  }
});

const selectTheme = (theme: ThemeInfo) => {
  // fromUserPicker 让主题可以声明首选明暗模式（如「微信摸鱼」默认浅色），
  // 冷启动与后端推送不会带该标记，因此不会覆盖用户手动切换的记忆。
  themeStore.applyThemeFile(theme.fileName, { fromUserPicker: true });
};

/** 无壁纸主题的缩略图回退：从主题变量里取色，画一张迷你聊天预览。 */
const previewVar = (theme: ThemeInfo, key: string, fallback: string) => {
  const vars = theme.variables.light || theme.variables.dark || {};
  return vars[key] || fallback;
};
</script>

<template>
  <div
    :class="
      props.layout === 'horizontal'
        ? 'flex overflow-x-auto space-x-4 pb-4 snap-x'
        : 'grid grid-cols-2 gap-4'
    "
  >
    <div
      v-for="theme in themeStore.availableThemes"
      :key="theme.fileName"
      @click="selectTheme(theme)"
      class="relative flex-shrink-0 cursor-pointer rounded-xl overflow-hidden border-2 transition-all group"
      :class="[
        props.layout === 'horizontal' ? 'w-48 h-32 snap-start' : 'aspect-video',
        themeStore.currentTheme === theme.fileName
          ? 'border-primary shadow-lg scale-95'
          : 'border-transparent opacity-80',
      ]"
    >
      <!-- Preview Image -->
      <div
        class="absolute inset-0 bg-cover bg-center transition-transform duration-500 group-hover:scale-110"
        :style="{
          backgroundImage: thumbnails[theme.fileName]
            ? `url(${thumbnails[theme.fileName]})`
            : 'none',
        }"
      >
        <!-- 无壁纸主题：用主题变量画迷你聊天预览，避免只剩一片灰底 -->
        <div
          v-if="!thumbnails[theme.fileName]"
          class="w-full h-full flex flex-col"
          :style="{
            backgroundColor: previewVar(theme, '--primary-bg', '#ededed'),
          }"
        >
          <div
            class="h-4 shrink-0"
            :style="{
              backgroundColor: previewVar(theme, '--secondary-bg', '#f7f7f7'),
            }"
          ></div>
          <div class="flex-1 flex flex-col justify-center gap-1.5 px-2">
            <div
              class="h-2.5 w-3/5 rounded-sm"
              :style="{
                backgroundColor: previewVar(
                  theme,
                  '--assistant-bubble-bg',
                  '#ffffff',
                ),
              }"
            ></div>
            <div
              class="h-2.5 w-2/5 rounded-sm self-end"
              :style="{
                backgroundColor: previewVar(
                  theme,
                  '--user-bubble-bg',
                  '#95ec69',
                ),
              }"
            ></div>
          </div>
        </div>
      </div>

      <!-- Overlays for visualization -->
      <div
        class="absolute inset-0 bg-black/20 group-hover:bg-black/10 transition-colors"
      ></div>

      <!-- Theme Name Tag -->
      <div
        class="absolute bottom-2 left-2 right-2 bg-black/40 rounded-lg px-2 py-1"
      >
        <p class="text-[10px] text-white font-medium truncate text-center">
          {{ theme.name }}
        </p>
      </div>

      <!-- Selection Indicator -->
      <div
        v-if="themeStore.currentTheme === theme.fileName"
        class="absolute top-2 left-2 bg-primary text-white p-1 rounded-full shadow-sm z-10 flex items-center justify-center"
      >
        <svg
          width="12"
          height="12"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="4"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <polyline points="20 6 9 17 4 12"></polyline>
        </svg>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* Hide scrollbar but allow scrolling */
.overflow-x-auto {
  scrollbar-width: none;
  -ms-overflow-style: none;
}

.overflow-y-auto::-webkit-scrollbar {
  display: none;
}
</style>
