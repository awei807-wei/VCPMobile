import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

export const useConnectionSwitchGuardStore = defineStore(
  "connectionSwitchGuard",
  () => {
    const switching = ref(false);

    const beginSwitch = async () => {
      switching.value = true;
      try {
        await invoke("begin_connection_profile_switch");
      } catch (error) {
        switching.value = false;
        throw error;
      }
    };

    const endSwitch = async () => {
      switching.value = false;
      try {
        await invoke("end_connection_profile_switch");
      } catch (error) {
        console.error(
          "[ConnectionSwitchGuard] Failed to end backend switch guard:",
          error,
        );
      }
    };

    return {
      switching,
      beginSwitch,
      endSwitch,
    };
  },
);
