import { invoke } from "@tauri-apps/api/core";
import { useAppLifecycleStore } from "../stores/appLifecycle";
import { useChatStreamStore } from "../stores/chatStreamStore";
import {
  generationKey,
  hydrateRecoveredStatus,
  isCoreNotReadyError,
  isValidGeneration,
  RecoveryChannelController,
  recoverActiveGeneration,
  resumeRecoveredGeneration,
  SharedStreamEventTail,
  type ActiveGeneration,
} from "./useAppLifecycleRecovery";
import type { StreamEvent } from "../types/chat";

type LifecycleStore = ReturnType<typeof useAppLifecycleStore>;
type StreamStore = ReturnType<typeof useChatStreamStore>;
type RecoveryAttempt = { success: boolean; deferred: boolean };
type PendingBoundary = { reason: RecoveryReason; sequence: number };
const MAX_RECOVERY_ATTEMPTS = 10;

export type RecoveryReason =
  | "mount"
  | "foreground"
  | "lifecycle-resume"
  | "network-online";

export interface RecoveryController {
  request(reason: RecoveryReason): Promise<void>;
  dispose(): void;
  isDisposed(): boolean;
  isPending(): boolean;
}

class RecoveryControllerImpl implements RecoveryController {
  private disposed = false;
  private lifecycleEpoch = 0;
  private recoveryPending = false;
  private recoveryPromise: Promise<void> | null = null;
  private readonly activeRecoveryKeys = new Set<string>();
  private readonly streamEventTail = new SharedStreamEventTail();
  private readonly channelControllers = new Set<RecoveryChannelController>();
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private retryAttempt = 0;
  private recoveryEpisodeActive = false;
  private recoveryEpisodeAttempts = 0;
  private recoveryEpisodeExhausted = false;
  private recoveryRequestSequence = 0;
  private pendingBoundary: PendingBoundary | null = null;

  constructor(
    private readonly lifecycleStore: LifecycleStore,
    private readonly streamStore: StreamStore,
  ) {}

  isDisposed() {
    return this.disposed;
  }

  isPending() {
    return this.recoveryPending;
  }

  dispose() {
    this.disposed = true;
    this.lifecycleEpoch += 1;
    this.recoveryPending = false;
    this.recoveryEpisodeActive = false;
    this.pendingBoundary = null;
    this.cancelRetryTimer();
    for (const controller of this.channelControllers) controller.dispose();
    this.channelControllers.clear();
  }

  private cancelRetryTimer(): void {
    if (this.retryTimer === null) return;
    clearTimeout(this.retryTimer);
    this.retryTimer = null;
  }

  private scheduleRetry(): void {
    if (
      this.disposed ||
      !this.recoveryPending ||
      this.lifecycleStore.state !== "READY" ||
      this.retryTimer !== null
    ) {
      return;
    }
    if (this.recoveryEpisodeAttempts >= MAX_RECOVERY_ATTEMPTS) {
      this.exhaustRecoveryEpisode();
      return;
    }
    const delay = Math.min(250 * 2 ** Math.min(this.retryAttempt, 5), 5_000);
    this.retryAttempt += 1;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      if (this.disposed || !this.recoveryPending) return;
      void this.startRecovery("foreground");
    }, delay);
  }

  private isEpisodeBoundary(reason: RecoveryReason): boolean {
    return (
      reason === "foreground" ||
      reason === "lifecycle-resume" ||
      reason === "network-online"
    );
  }

  private beginRecoveryEpisode(reason: RecoveryReason): void {
    if (this.recoveryEpisodeExhausted && !this.isEpisodeBoundary(reason)) {
      return;
    }
    if (!this.recoveryEpisodeActive || this.isEpisodeBoundary(reason)) {
      this.recoveryEpisodeActive = true;
      this.recoveryEpisodeAttempts = 0;
      this.recoveryEpisodeExhausted = false;
      this.retryAttempt = 0;
    }
    this.cancelRetryTimer();
  }

  private exhaustRecoveryEpisode(): void {
    if (this.recoveryEpisodeExhausted) return;
    this.recoveryEpisodeExhausted = true;
    this.recoveryEpisodeActive = false;
    this.recoveryPending = false;
    this.cancelRetryTimer();
    console.error(
      `[useAppLifecycle] 核心未就绪，恢复尝试已达到上限（${MAX_RECOVERY_ATTEMPTS} 次），放弃本次恢复。`,
    );
  }

  private startPendingBoundary(): boolean {
    const pending = this.pendingBoundary;
    if (!pending) return false;
    this.pendingBoundary = null;
    this.recoveryPending = true;
    this.beginRecoveryEpisode(pending.reason);
    console.log(
      `[useAppLifecycle] 处理挂起的恢复边界：${pending.reason}（序号 ${pending.sequence}），开启新 episode。`,
    );
    void this.startRecovery(pending.reason);
    return true;
  }

  private finishRecoveryAttempt(): void {
    this.recoveryPromise = null;
    if (this.startPendingBoundary()) return;
    if (this.recoveryPending) this.scheduleRetry();
    else this.recoveryEpisodeActive = false;
  }

  private isCurrent(epoch: number): boolean {
    return !this.disposed && epoch === this.lifecycleEpoch;
  }

  async recoverOneGeneration(
    generation: ActiveGeneration,
  ): Promise<RecoveryAttempt> {
    const epoch = this.lifecycleEpoch;
    if (!this.isCurrent(epoch) || this.lifecycleStore.state !== "READY") {
      this.recoveryPending = !this.disposed;
      console.log("[useAppLifecycle] 核心离开 READY，延后剩余恢复任务。");
      return { success: false, deferred: true };
    }
    const recoveryKey = generationKey(generation);
    if (this.activeRecoveryKeys.has(recoveryKey)) {
      return { success: true, deferred: false };
    }
    this.activeRecoveryKeys.add(recoveryKey);
    try {
      const recovery = await recoverActiveGeneration(generation);
      if (!this.isCurrent(epoch)) {
        return { success: false, deferred: true };
      }
      if (recovery.deferred) {
        this.recoveryPending = !this.disposed;
        return { success: false, deferred: true };
      }
      const status = recovery.status;
      if (!status) return { success: true, deferred: false };
      const statusName = status.status?.toLowerCase();
      if (statusName === "streaming") {
        return {
          success: await this.resumeGeneration(generation, status, epoch),
          deferred: false,
        };
      }
      if (statusName === "completed" || statusName === "failed") {
        return {
          success: await hydrateRecoveredStatus(
            generation,
            status,
            this.streamStore,
            this.streamEventTail,
            () => this.isCurrent(epoch),
          ),
          deferred: false,
        };
      }
      return { success: this.isCurrent(epoch), deferred: false };
    } finally {
      this.activeRecoveryKeys.delete(recoveryKey);
    }
  }

  private async resumeGeneration(
    generation: ActiveGeneration,
    status: NonNullable<
      Awaited<ReturnType<typeof recoverActiveGeneration>>["status"]
    >,
    epoch: number,
  ): Promise<boolean> {
    if (!this.isCurrent(epoch)) return false;
    let controller: RecoveryChannelController;
    const processEvent = async (event: StreamEvent): Promise<void> => {
      try {
        await this.streamStore.processStreamEvent(event, {
          countMessage: false,
        });
      } finally {
        if (event.type === "end" || event.type === "error") {
          this.finishChannelController(controller);
        }
      }
    };
    controller = new RecoveryChannelController(
      processEvent,
      this.streamEventTail,
    );
    this.channelControllers.add(controller);
    const resumed = await resumeRecoveredGeneration(
      generation,
      status,
      this.streamStore,
      controller,
    );
    if (!resumed || !this.isCurrent(epoch)) {
      controller.dispose();
      this.channelControllers.delete(controller);
      return false;
    }
    return true;
  }

  private finishChannelController(controller: RecoveryChannelController): void {
    if (!this.channelControllers.delete(controller)) return;
    controller.dispose();
  }

  async recoverInterruptedStreams(epoch = this.lifecycleEpoch) {
    try {
      if (!this.isCurrent(epoch) || this.lifecycleStore.state !== "READY") {
        this.recoveryPending = !this.disposed;
        return;
      }
      const activeGenerations = await invoke<ActiveGeneration[]>(
        "get_active_generations",
      );
      if (!this.isCurrent(epoch)) return;
      if (this.lifecycleStore.state !== "READY") {
        this.recoveryPending = !this.disposed;
        return;
      }
      if (!activeGenerations?.length) {
        console.log("[useAppLifecycle] 没有需要恢复的活动生成。");
        if (!this.pendingBoundary) {
          this.recoveryPending = false;
          this.recoveryEpisodeActive = false;
          this.retryAttempt = 0;
          this.cancelRetryTimer();
        }
        return;
      }
      console.log(
        `[useAppLifecycle] 发现 ${activeGenerations.length} 个活动生成，开始恢复。`,
      );
      const uniqueGenerations = new Map<string, ActiveGeneration>();
      for (const generation of activeGenerations) {
        if (!this.isCurrent(epoch)) return;
        if (!isValidGeneration(generation)) {
          console.warn(
            "[useAppLifecycle] 忽略缺少完整身份的生成记录：",
            generation,
          );
          continue;
        }
        uniqueGenerations.set(generationKey(generation), generation);
      }
      const results = await Promise.all(
        [...uniqueGenerations.values()].map((generation) =>
          this.recoverOneGeneration(generation),
        ),
      );
      if (!this.isCurrent(epoch)) return;
      if (results.some((result) => result.deferred)) {
        this.recoveryPending = !this.disposed;
      }
    } catch (error) {
      if (isCoreNotReadyError(error)) {
        console.info("[useAppLifecycle] 核心尚未就绪，延后流恢复。");
        this.recoveryPending = !this.disposed;
        return;
      }
      console.error("[useAppLifecycle] 获取活动生成失败：", error);
    }
  }

  request(reason: RecoveryReason): Promise<void> {
    const sequence = ++this.recoveryRequestSequence;
    if (this.recoveryPromise) {
      if (this.isEpisodeBoundary(reason)) {
        this.pendingBoundary = { reason, sequence };
        this.recoveryPending = true;
        console.log(
          `[useAppLifecycle] 恢复任务正在运行，记录边界 ${reason}（序号 ${sequence}），等待当前 episode 结束。`,
        );
      } else {
        this.recoveryPending = true;
        console.log(`[useAppLifecycle] 恢复任务正在运行，合并触发：${reason}`);
      }
      return this.recoveryPromise;
    }
    this.beginRecoveryEpisode(reason);
    return this.startRecovery(reason);
  }

  private startRecovery(reason: RecoveryReason): Promise<void> {
    if (this.disposed) return Promise.resolve();
    if (this.recoveryPromise) {
      this.recoveryPending = true;
      console.log(`[useAppLifecycle] 恢复任务正在运行，合并触发：${reason}`);
      return this.recoveryPromise;
    }
    if (this.recoveryEpisodeExhausted) return Promise.resolve();
    this.recoveryPending = true;
    if (this.lifecycleStore.state !== "READY") {
      console.log(
        `[useAppLifecycle] 核心当前为 ${this.lifecycleStore.state}，延后恢复触发：${reason}`,
      );
      return Promise.resolve();
    }
    if (this.recoveryEpisodeAttempts >= MAX_RECOVERY_ATTEMPTS) {
      this.exhaustRecoveryEpisode();
      return Promise.resolve();
    }
    this.recoveryEpisodeAttempts += 1;
    this.recoveryPending = false;
    const epoch = this.lifecycleEpoch;
    this.recoveryPromise = this.recoverInterruptedStreams(epoch).finally(() => {
      this.finishRecoveryAttempt();
    });
    return this.recoveryPromise;
  }
}

export function createRecoveryController(
  lifecycleStore: LifecycleStore,
  streamStore: StreamStore,
): RecoveryController {
  return new RecoveryControllerImpl(lifecycleStore, streamStore);
}
