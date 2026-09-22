<script setup lang="ts">
import { computed } from "vue";
import { useNotificationStore } from "../../core/stores/notification";

const store = useNotificationStore();

const statusTone = computed(() => {
  switch (store.vcpStatus.status) {
    case "connected":
      return "is-connected";
    case "disconnected":
    case "closed":
      return "is-disconnected";
    case "error":
      return "is-error";
    case "connecting":
      return "is-connecting";
    default:
      return "is-idle";
  }
});
</script>

<template>
  <div
    class="vcp-notification-status w-full text-center py-1.5 text-[10px] font-black uppercase tracking-[0.2em] transition-colors duration-300 shadow-sm relative z-20"
    :class="statusTone"
  >
    {{ store.vcpStatus.source || "VCPLog" }}:
    {{ store.vcpStatus.message || "IDLE" }}
  </div>
</template>

<style scoped>
.vcp-notification-status {
  color: var(--primary-text);
  background-color: var(--notification-header-bg, var(--secondary-bg));
  border-bottom: 1px solid
    color-mix(
      in srgb,
      var(--notification-border, var(--border-color)) 55%,
      transparent
    );
}

.vcp-notification-status.is-connected {
  color: var(--text-on-accent, #fff);
  background-color: color-mix(
    in srgb,
    var(--success-color, #2e7d32) 82%,
    var(--notification-header-bg, var(--secondary-bg))
  );
}

.vcp-notification-status.is-disconnected,
.vcp-notification-status.is-error {
  color: var(--text-on-accent, #fff);
  background-color: color-mix(
    in srgb,
    var(--danger-color, #c62828) 84%,
    var(--notification-header-bg, var(--secondary-bg))
  );
}

.vcp-notification-status.is-connecting {
  color: var(--text-on-accent, #fff);
  background-color: color-mix(
    in srgb,
    var(--highlight-text, #f9a825) 78%,
    var(--notification-header-bg, var(--secondary-bg))
  );
}
</style>
