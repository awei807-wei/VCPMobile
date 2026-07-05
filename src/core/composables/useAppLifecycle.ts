import { onMounted, onUnmounted, watch } from 'vue';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { useAppLifecycleStore } from '../stores/appLifecycle';

export function useAppLifecycle() {
  const lifecycleStore = useAppLifecycleStore();
  let unlisten: UnlistenFn | null = null;

  // 检测是否是划词助手窗口
  const isAssistant =
    typeof window !== 'undefined' && window.location.search.includes('mode=floating');

  // 监听后台状态，控制全局动画挂起
  watch(
    () => lifecycleStore.isBackground,
    (newVal) => {
      if (isAssistant) return;

      if (newVal) {
        document.documentElement.classList.add('vcp-paused-animations');
        console.log('[useAppLifecycle] App moved to background, pausing animations.');
      } else {
        document.documentElement.classList.remove('vcp-paused-animations');
        console.log('[useAppLifecycle] App moved to foreground, resuming animations.');
        // 返回前台时检查并恢复中断的流
        checkAndRecoverInterruptedStreams().catch((err) => {
          console.error('[useAppLifecycle] Failed to recover streams on resume:', err);
        });
      }
    },
    { immediate: true },
  );

  const handleVisibilityChange = () => {
    if (typeof document !== 'undefined') {
      const isHidden = document.hidden;
      lifecycleStore.isBackground = isHidden;
      console.log(`[useAppLifecycle] Visibility changed: hidden=${isHidden}`);
    }
  };

  const handleOnline = () => {
    console.log('[useAppLifecycle] Device online. Triggering stream recovery.');
    checkAndRecoverInterruptedStreams().catch((err) => {
      console.error('[useAppLifecycle] Failed to recover streams on network restored:', err);
    });
  };

  /**
   * 检查并恢复中断的流式生成。
   * 调用 Rust 后端 get_active_generations 获取活跃生成列表，
   * 对每个非活跃的生成调用 recover_active_generation 恢复。
   */
  const checkAndRecoverInterruptedStreams = async () => {
    try {
      const activeGenerations = await invoke<
        Array<{
          msgId: string;
          topicId: string;
          ownerId: string;
          ownerType: string;
          createdAt: number;
        }>
      >('get_active_generations');

      if (!activeGenerations || activeGenerations.length === 0) {
        console.log('[useAppLifecycle] No active generations to recover.');
        return;
      }

      console.log(
        `[useAppLifecycle] Found ${activeGenerations.length} active generations, attempting recovery...`,
      );

      for (const gen of activeGenerations) {
        try {
          console.log(
            `[useAppLifecycle] Recovering generation: msgId=${gen.msgId}, topicId=${gen.topicId}`,
          );
          await invoke('recover_active_generation', {
            msgId: gen.msgId,
          });
        } catch (err) {
          console.warn(
            `[useAppLifecycle] Failed to recover generation ${gen.msgId}:`,
            err,
          );
        }
      }
    } catch (err) {
      console.error('[useAppLifecycle] Failed to get active generations:', err);
    }
  };

  onMounted(async () => {
    if (typeof window !== 'undefined') {
      document.addEventListener('visibilitychange', handleVisibilityChange);
      window.addEventListener('online', handleOnline);
    }

    try {
      unlisten = await listen<{ state: string }>('vcp-lifecycle-changed', (event) => {
        const state = event.payload.state;
        console.log(`[useAppLifecycle] Received vcp-lifecycle-changed: state=${state}`);

        if (state === 'pause' || state === 'stop') {
          lifecycleStore.isBackground = true;
        } else if (state === 'resume') {
          lifecycleStore.isBackground = false;
          checkAndRecoverInterruptedStreams().catch((err) => {
            console.error('[useAppLifecycle] Failed to recover streams on resume:', err);
          });
        }
      });
    } catch (err) {
      console.error('[useAppLifecycle] Failed to setup Tauri lifecycle listener:', err);
    }

    // 启动时检查是否有需要恢复的流
    checkAndRecoverInterruptedStreams().catch((err) => {
      console.error('[useAppLifecycle] Failed to recover streams on mount:', err);
    });
  });

  onUnmounted(() => {
    if (typeof window !== 'undefined') {
      document.removeEventListener('visibilitychange', handleVisibilityChange);
      window.removeEventListener('online', handleOnline);
    }
    if (unlisten) {
      unlisten();
    }
  });
}
