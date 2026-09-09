import { Channel, invoke } from "@tauri-apps/api/core";
import { useChatStreamStore } from "../stores/chatStreamStore";
import type { StreamEvent } from "../types/chat";

export interface ActiveGeneration {
  msgId: string;
  topicId: string;
  ownerId: string;
  ownerType: "agent" | "group";
  createdAt: number;
  helperGeneration?: number | null;
}

export interface RecoveryStatus {
  status?: string;
  content?: string;
  snapshot?: unknown;
  contentSnapshot?: unknown;
  fullContent?: string;
  error?: string;
  finishReason?: string;
  helperGeneration?: number | null;
  generation?: number | null;
  lastEventIndex?: number | null;
  last_event_index?: number | null;
}

export interface RecoveryResult {
  status: RecoveryStatus | null;
  deferred: boolean;
}

export function isCoreNotReadyError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  return message.includes("CORE_NOT_READY:");
}

export function generationKey(generation: ActiveGeneration) {
  return JSON.stringify([
    generation.ownerType,
    generation.ownerId,
    generation.topicId,
    generation.msgId,
  ]);
}

export function isValidGeneration(value: ActiveGeneration) {
  return (
    !!value &&
    !!value.msgId &&
    !!value.topicId &&
    !!value.ownerId &&
    (value.ownerType === "agent" || value.ownerType === "group")
  );
}

export function readHelperGeneration(recovery: RecoveryStatus): number | null {
  const value = recovery.helperGeneration ?? recovery.generation;
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0
    ? value
    : null;
}

/** 兼容 0010 之前的字段名；返回值只允许进入 helper resume 契约。 */
export function readRecoveryGeneration(recovery: RecoveryStatus): number | null {
  return readHelperGeneration(recovery);
}

export type StreamEventProcessor = (event: StreamEvent) => Promise<unknown>;

/** 跨恢复 key 共享的事件处理尾，避免并发 Channel 交错修改 UI。 */
export class SharedStreamEventTail {
  private tail: Promise<unknown> = Promise.resolve();

  enqueue(
    event: StreamEvent,
    isActive: () => boolean,
    process: StreamEventProcessor,
  ): Promise<void> {
    const run = this.tail.catch(() => undefined).then(async () => {
      if (!isActive()) return;
      await process(event);
    });
    this.tail = run.catch(() => undefined);
    return run;
  }

  enqueueBatch(
    events: StreamEvent[],
    isActive: () => boolean,
    process: StreamEventProcessor,
  ): Promise<void> {
    const run = this.tail.catch(() => undefined).then(async () => {
      if (!isActive()) return;
      for (const event of events) {
        if (!isActive()) return;
        await process(event);
      }
    });
    this.tail = run.catch(() => undefined);
    return run;
  }
}

/** Channel 的 generation/disposed 门禁；卸载后已排队事件全部丢弃。 */
export class RecoveryChannelController {
  readonly channel = new Channel<StreamEvent>();
  private epoch = 0;
  private disposed = false;

  constructor(
    process: StreamEventProcessor,
    private readonly tail: SharedStreamEventTail = new SharedStreamEventTail(),
  ) {
    this.channel.onmessage = (event) => {
      const eventEpoch = this.epoch;
      if (this.disposed) return;
      void this.tail
        .enqueue(event, () => !this.disposed && eventEpoch === this.epoch, process)
        .catch((error) => {
          console.error("[useAppLifecycle] 恢复流事件处理失败：", error);
        });
    };
  }

  dispose() {
    this.disposed = true;
    this.epoch += 1;
  }

  isDisposed(): boolean {
    return this.disposed;
  }
}

export async function recoverActiveGeneration(
  generation: ActiveGeneration,
): Promise<RecoveryResult> {
  try {
    console.log(
      `[useAppLifecycle] 恢复流式生成：${generation.ownerType}/${generation.ownerId}/${generation.topicId}/${generation.msgId}`,
    );
    const status = await invoke<RecoveryStatus>("recover_active_generation", {
      msgId: generation.msgId,
      ownerId: generation.ownerId,
      ownerType: generation.ownerType,
      topicId: generation.topicId,
    });
    return { status: status || null, deferred: false };
  } catch (error) {
    if (isCoreNotReadyError(error)) {
      console.info(
        `[useAppLifecycle] 核心尚未就绪，延后恢复：${generation.msgId}`,
      );
      return { status: null, deferred: true };
    }
    console.warn(
      `[useAppLifecycle] 恢复生成失败（${generation.msgId}）：`,
      error,
    );
    return { status: null, deferred: false };
  }
}

export async function resumeRecoveredGeneration(
  generation: ActiveGeneration,
  recovery: RecoveryStatus,
  streamStore: ReturnType<typeof useChatStreamStore>,
  controller?: RecoveryChannelController,
): Promise<boolean> {
  const expectedGeneration = readHelperGeneration(recovery);
  if (expectedGeneration === null) {
    console.error(
      `[useAppLifecycle] streaming 恢复缺少正整数 generation，拒绝启动：${generation.msgId}`,
    );
    return false;
  }
  const channelController =
    controller ||
    new RecoveryChannelController((event) =>
      streamStore.processStreamEvent(event, { countMessage: false }),
    );
  const streamChannel = channelController.channel;

  const initialContent =
    typeof recovery.content === "string" ? recovery.content : undefined;
  const lastEventIndex =
    typeof recovery.lastEventIndex === "number"
      ? recovery.lastEventIndex
      : typeof recovery.last_event_index === "number"
        ? recovery.last_event_index
        : undefined;
  const resumePayload: Record<string, unknown> = {
    msgId: generation.msgId,
    topicId: generation.topicId,
    ownerId: generation.ownerId,
    ownerType: generation.ownerType,
    expectedGeneration,
    streamChannel,
  };
  if (initialContent !== undefined)
    resumePayload.initialContent = initialContent;
  if (lastEventIndex !== undefined)
    resumePayload.lastEventIndex = lastEventIndex;

  try {
    await invoke("resume_stream", resumePayload);
    if (channelController.isDisposed()) return false;
    return true;
  } catch (error) {
    if (!controller) channelController.dispose();
    console.error(
      `[useAppLifecycle] 接续流失败（${generation.ownerType}/${generation.ownerId}/${generation.topicId}/${generation.msgId}）：`,
      error,
    );
    return false;
  }
}

function recoveryEventContext(generation: ActiveGeneration) {
  return {
    topicId: generation.topicId,
    ownerId: generation.ownerId,
    ownerType: generation.ownerType,
  };
}

function readHydrationContent(recovery: RecoveryStatus): string | null {
  const direct = [recovery.content, recovery.fullContent];
  for (const value of direct) {
    if (typeof value === "string" && value.length > 0) return value;
  }
  const snapshots = [recovery.snapshot, recovery.contentSnapshot];
  for (const snapshot of snapshots) {
    if (typeof snapshot === "string" && snapshot.length > 0) return snapshot;
    if (!snapshot || typeof snapshot !== "object") continue;
    const content = (snapshot as Record<string, unknown>).content;
    if (typeof content === "string" && content.length > 0) return content;
  }
  return null;
}

function addRecoveryGeneration(
  event: StreamEvent,
  generation: number,
): StreamEvent {
  return {
    ...event,
    generation,
    streamGeneration: generation,
  } as StreamEvent;
}

function appendRecoverySnapshot(
  events: StreamEvent[],
  generation: ActiveGeneration,
  recovery: RecoveryStatus,
  recoveryGeneration: number,
  context: ReturnType<typeof recoveryEventContext>,
): void {
  const snapshot = recovery.snapshot ?? recovery.contentSnapshot;
  if (snapshot === undefined) return;
  if (!Array.isArray(snapshot)) {
    console.warn(
      `[useAppLifecycle] 终态恢复快照格式无效，跳过快照：${generation.msgId}`,
    );
    return;
  }
  events.push(
    addRecoveryGeneration(
      {
        type: "aurora",
        messageId: generation.msgId,
        context,
        aurora: { tailSnapshot: snapshot },
      },
      recoveryGeneration,
    ),
  );
}

function buildHydrationEvents(
  generation: ActiveGeneration,
  recovery: RecoveryStatus,
  content: string,
  recoveryGeneration: number,
): StreamEvent[] {
  const context = recoveryEventContext(generation);
  const events: StreamEvent[] = [
    addRecoveryGeneration(
      { type: "data", messageId: generation.msgId, context, chunk: content },
      recoveryGeneration,
    ),
  ];
  appendRecoverySnapshot(events, generation, recovery, recoveryGeneration, context);
  const status = recovery.status?.toLowerCase();
  events.push(
    addRecoveryGeneration(
      {
        type: status === "failed" ? "error" : "end",
        messageId: generation.msgId,
        context,
        error:
          status === "failed" ? recovery.error || "恢复记录标记为失败" : undefined,
        finishReason:
          recovery.finishReason || (status === "failed" ? "error" : "completed"),
      },
      recoveryGeneration,
    ),
  );
  return events;
}

/** 使用非持久化 stream store 路径同步已终态恢复记录，拒绝空消息骨架。 */
export async function hydrateRecoveredStatus(
  generation: ActiveGeneration,
  recovery: RecoveryStatus,
  streamStore: ReturnType<typeof useChatStreamStore>,
  tail: SharedStreamEventTail = new SharedStreamEventTail(),
  isActive: () => boolean = () => true,
): Promise<boolean> {
  const content = readHydrationContent(recovery);
  if (!content) {
    console.warn(
      `[useAppLifecycle] 终态恢复缺少内容，跳过空消息骨架：${generation.msgId}`,
    );
    return false;
  }
  const allocatedGeneration =
    typeof streamStore.allocateStreamGeneration === "function"
      ? streamStore.allocateStreamGeneration(
          generation.ownerId,
          generation.ownerType,
          generation.topicId,
          generation.msgId,
        )
      : readRecoveryGeneration(recovery);
  if (allocatedGeneration === null || allocatedGeneration === 0) {
    console.error(
      `[useAppLifecycle] 终态恢复缺少正整数 generation（无法分配可比较前端序列），拒绝写入：${generation.msgId}`,
    );
    return false;
  }
  const events = buildHydrationEvents(
    generation,
    recovery,
    content,
    allocatedGeneration,
  );
  await tail.enqueueBatch(events, isActive, (event) =>
    streamStore.processStreamEvent(event, { countMessage: false }),
  );
  return isActive();
}
