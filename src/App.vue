<script setup lang="ts">
import PermissionGate from "./components/layout/PermissionGate.vue";
import BootScreen from "./components/layout/BootScreen.vue";
import AgentSidebar from "./components/layout/AgentSidebar.vue";
import RightSidebar from "./components/layout/RightSidebar.vue";
import GlobalOverlayManager from "./components/GlobalOverlayManager.vue";
import FeatureOverlays from "./components/FeatureOverlays.vue";
import UpdatePrompt from "./components/ui/UpdatePrompt.vue";
import ShareAgentSelector from "./features/chat/components/ShareAgentSelector.vue";
import { useAppShell } from "./core/composables/useAppShell";

const {
  appRootRef,
  lifecycleStore,
  themeStore,
  layoutStore,
  backgroundStyle,
  sharedContent,
  showShareSelector,
  handleShareAgentSelected,
  handleShareSelectorClose,
  isPromptOpen,
  updateInfo,
  handleConfirm,
  handleDismiss,
} = useAppShell();
</script>

<template>
  <div
    ref="appRootRef"
    class="vcp-app-root h-full w-full overflow-hidden flex flex-col select-none relative"
  >
    <PermissionGate v-if="lifecycleStore.state === 'PERMISSIONS'" />
    <BootScreen v-else />

    <Transition name="bg-fade">
      <div
        :key="backgroundStyle.backgroundImage"
        class="vcp-background-layer"
        :style="backgroundStyle"
      ></div>
    </Transition>
    <div
      class="vcp-background-overlay absolute inset-0 pointer-events-none transition-colors"
      style="transition-duration: 350ms"
      :class="themeStore.isDarkResolved ? 'bg-black/12' : 'bg-transparent'"
    ></div>

    <main class="flex-1 min-w-0 relative overflow-hidden">
      <router-view v-slot="{ Component }">
        <component
          v-if="Component && lifecycleStore.state === 'READY'"
          :is="Component"
        />
      </router-view>
    </main>

    <Transition name="fade">
      <div
        v-if="layoutStore.leftDrawerOpen || layoutStore.rightDrawerOpen"
        class="vcp-overlay fixed inset-0 z-drawer bg-black/12 md:hidden"
        @click.self="
          layoutStore.setLeftDrawer(false);
          layoutStore.setRightDrawer(false);
        "
      ></div>
    </Transition>

    <AgentSidebar v-if="lifecycleStore.state === 'READY'" />
    <RightSidebar
      v-if="lifecycleStore.state === 'READY'"
      class="pointer-events-auto shrink-0"
      :is-open="layoutStore.rightDrawerOpen"
      @close="layoutStore.setRightDrawer(false)"
    />

    <GlobalOverlayManager v-if="lifecycleStore.state === 'READY'" />
    <FeatureOverlays v-if="lifecycleStore.state === 'READY'" />

    <ShareAgentSelector
      v-if="lifecycleStore.state === 'READY'"
      :is-open="showShareSelector"
      :shared-text="sharedContent.text"
      :shared-file-count="sharedContent.files.length"
      @close="handleShareSelectorClose"
      @selected="handleShareAgentSelected"
    />

    <UpdatePrompt
      v-model:is-open="isPromptOpen"
      :version="updateInfo?.latestVersion || ''"
      :release-notes="updateInfo?.releaseNotes"
      :apk-size="updateInfo?.apkSize"
      :release-page-url="updateInfo?.releasePageUrl"
      @confirm="handleConfirm"
      @dismiss="handleDismiss"
    />
  </div>
</template>

<style src="./App.css"></style>
