import { useAppLifecycleStore } from "../stores/appLifecycle";
import { useChatStreamStore } from "../stores/chatStreamStore";
import { installAppLifecycleRuntime } from "./useAppLifecycleRuntime";

export function useAppLifecycle() {
  installAppLifecycleRuntime(useAppLifecycleStore(), useChatStreamStore());
}
