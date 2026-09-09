// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useChatStreamStore } from "../../../core/stores/chatStreamStore";
import { parseStreamEvent } from "../../../core/stores/chatStreamProcessorSupport";
import { mockInvoke } from "../../mocks/tauri";
import type { MarkdownNode, StreamEvent } from "../../../core/types/chat";

const streamContext = {
  ownerId: "agent-a",
  ownerType: "agent" as const,
  topicId: "topic-a",
  agentId: "agent-a",
};

const streamEvent = (
  event: Partial<StreamEvent> & Pick<StreamEvent, "type" | "messageId">,
): StreamEvent => ({
  generation: 1,
  context: streamContext,
  ...event,
});

function tailEvent(
  frameSeq: number,
  text: string,
  options: { epoch?: number; reset?: boolean; chunk?: string } = {},
): StreamEvent {
  const snapshot: MarkdownNode[] = [
    { type: "paragraph", children: [{ type: "text", value: text }] },
  ];
  return streamEvent({
    type: "aurora",
    messageId: "assistant-1",
    aurora: {
      chunk: options.chunk ?? text,
      tailChanged: true,
      tail: text,
      tailBlock: {
        type: "markdown",
        content: text,
        nodes: snapshot,
        hash: `${frameSeq}`,
      },
      tailFrame: {
        epoch: options.epoch ?? 1,
        revision: frameSeq,
        frameSeq,
        reset: options.reset,
        snapshot: options.reset ? snapshot : undefined,
        mutations: options.reset
          ? []
          : [{ op: "append", id: "t0.i0", chunk: text }],
      },
    },
  });
}

function installManualRaf() {
  const originalRaf = window.requestAnimationFrame;
  const originalCancelRaf = window.cancelAnimationFrame;
  const callbacks = new Map<number, FrameRequestCallback>();
  let nextId = 1;
  let now = 100;
  const nowSpy = vi.spyOn(performance, "now").mockImplementation(() => now);
  window.requestAnimationFrame = vi.fn((callback: FrameRequestCallback) => {
    const id = nextId++;
    callbacks.set(id, callback);
    return id;
  });
  window.cancelAnimationFrame = vi.fn((id: number) => {
    callbacks.delete(id);
  });
  return {
    flush() {
      now += 100;
      const queued = [...callbacks.values()];
      callbacks.clear();
      queued.forEach((callback) => callback(now));
    },
    restore() {
      nowSpy.mockRestore();
      window.requestAnimationFrame = originalRaf;
      window.cancelAnimationFrame = originalCancelRaf;
    },
  };
}

describe("流式渲染背压与纪元门禁", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockInvoke("process_message_content", () => []);
  });

  it("群聊 skeleton 使用请求上下文中的发言 Agent 身份", async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent({
      type: "thinking",
      generation: 1,
      messageId: "group-message",
      context: {
        groupId: "group-a",
        ownerType: "group",
        topicId: "group-topic",
        speakerAgentId: "speaker-a",
        isGroupMessage: true,
        agentName: "Speaker",
      },
    });

    expect(
      store.getActiveStreamMessage(
        "group-a",
        "group",
        "group-topic",
        "group-message",
      ),
    ).toMatchObject({
      agentId: "speaker-a",
      groupId: "group-a",
      isGroupMessage: true,
      name: "Speaker",
    });
  });

  it.each(["agentId", "agent_id"] as const)(
    "单聊 skeleton 保留 %s 上下文字段兼容",
    async (agentIdField) => {
      const store = useChatStreamStore();
      await store.processStreamEvent({
        type: "thinking",
        generation: 1,
        messageId: "agent-message",
        context: {
          ownerId: "agent-a",
          ownerType: "agent",
          topicId: "agent-topic",
          [agentIdField]: "agent-a",
        },
      });

      expect(
        store.getActiveStreamMessage(
          "agent-a",
          "agent",
          "agent-topic",
          "agent-message",
        )?.agentId,
      ).toBe("agent-a");
    },
  );

  it("将新 generation 视为新流，并拒绝旧 generation 的迟到正文", async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(
      streamEvent({ type: "thinking", messageId: "assistant-1" }),
    );
    await store.processStreamEvent(
      streamEvent({
        type: "data",
        messageId: "assistant-1",
        generation: 1,
        chunk: "旧",
      }),
    );
    await store.processStreamEvent(
      streamEvent({
        type: "thinking",
        messageId: "assistant-1",
        generation: 2,
      }),
    );
    await store.processStreamEvent(
      streamEvent({
        type: "data",
        messageId: "assistant-1",
        generation: 2,
        chunk: "新",
      }),
    );
    await store.processStreamEvent(
      streamEvent({
        type: "data",
        messageId: "assistant-1",
        generation: 1,
        chunk: "迟到",
      }),
    );
    expect(
      store.getActiveStreamMessage("agent-a", "agent", "topic-a", "assistant-1")
        ?.content,
    ).toBe("新");
  });

  it("丢弃重复 frame，避免同一 chunk 被追加两次", async () => {
    const raf = installManualRaf();
    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(
        streamEvent({ type: "thinking", messageId: "assistant-1" }),
      );
      await store.processStreamEvent(tailEvent(1, "一次", { reset: true }));
      raf.flush();
      await store.processStreamEvent(tailEvent(1, "重复", { chunk: "重复" }));
      expect(
        store.getActiveStreamMessage(
          "agent-a",
          "agent",
          "topic-a",
          "assistant-1",
        )?.content,
      ).toBe("一次");
    } finally {
      raf.restore();
    }
  });

  it("发现 frame gap 时切换到最新完整快照", async () => {
    const raf = installManualRaf();
    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(
        streamEvent({ type: "thinking", messageId: "assistant-1" }),
      );
      await store.processStreamEvent(tailEvent(1, "一", { reset: true }));
      raf.flush();
      await store.processStreamEvent(tailEvent(3, "三"));
      await store.processStreamEvent(
        streamEvent({ type: "end", messageId: "assistant-1" }),
      );
      const frame = store.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-1",
      )?.tailFrame;
      expect(frame?.reset).toBe(true);
      expect(frame?.mutations).toEqual([]);
      expect(frame?.snapshot?.[0]).toMatchObject({
        children: [{ value: "三" }],
      });
    } finally {
      raf.restore();
    }
  });

  it("限制待刷新的 mutation 数量并保留顺序", async () => {
    const raf = installManualRaf();
    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(
        streamEvent({ type: "thinking", messageId: "assistant-1" }),
      );
      for (let frameSeq = 1; frameSeq <= 513; frameSeq += 1)
        await store.processStreamEvent(tailEvent(frameSeq, String(frameSeq)));
      const message = store.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-1",
      );
      expect(message?.content).toBe("");
      await store.processStreamEvent(
        streamEvent({ type: "end", messageId: "assistant-1" }),
      );
      expect(message?.tailFrame?.mutations).toEqual([]);
      expect(message?.tailFrame?.reset).toBe(true);
      expect(message?.tailFrame?.snapshot?.[0]).toMatchObject({
        children: [{ value: "513" }],
      });
    } finally {
      raf.restore();
    }
  });

  it("隐藏 WebView 只保留最新快照", async () => {
    const originalRaf = window.requestAnimationFrame;
    const hiddenDescriptor = Object.getOwnPropertyDescriptor(
      document,
      "hidden",
    );
    Object.defineProperty(document, "hidden", {
      configurable: true,
      value: true,
    });
    window.requestAnimationFrame = vi.fn(() => 1);
    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(
        streamEvent({ type: "thinking", messageId: "assistant-1" }),
      );
      for (let frameSeq = 1; frameSeq <= 700; frameSeq += 1)
        await store.processStreamEvent(tailEvent(frameSeq, `节点-${frameSeq}`));
      await store.processStreamEvent(
        streamEvent({ type: "end", messageId: "assistant-1" }),
      );
      const frame = store.getActiveStreamMessage(
        "agent-a",
        "agent",
        "topic-a",
        "assistant-1",
      )?.tailFrame;
      expect(frame?.reset).toBe(true);
      expect(frame?.mutations).toEqual([]);
      expect(frame?.snapshot?.[0]).toMatchObject({
        children: [{ value: "节点-700" }],
      });
    } finally {
      window.requestAnimationFrame = originalRaf;
      if (hiddenDescriptor)
        Object.defineProperty(document, "hidden", hiddenDescriptor);
    }
  });

  it("terminal 会强制刷出 pending frame 并清理 generation", async () => {
    const raf = installManualRaf();
    try {
      const store = useChatStreamStore();
      await store.processStreamEvent(
        streamEvent({ type: "thinking", messageId: "assistant-1" }),
      );
      await store.processStreamEvent(tailEvent(1, "终态", { reset: true }));
      await store.processStreamEvent(
        streamEvent({ type: "end", messageId: "assistant-1" }),
      );
      expect(
        store.getActiveStreamMessage(
          "agent-a",
          "agent",
          "topic-a",
          "assistant-1",
        )?.content,
      ).toBe("终态");
      expect(store.streamGenerations.size).toBe(0);
    } finally {
      raf.restore();
    }
  });

  it("终态保留 generation 水位并拒绝迟到 thinking、正文和终态", async () => {
    const store = useChatStreamStore();
    await store.processStreamEvent(
      streamEvent({
        type: "thinking",
        messageId: "assistant-1",
        generation: 7,
      }),
    );
    await store.processStreamEvent(
      streamEvent({
        type: "data",
        messageId: "assistant-1",
        generation: 7,
        chunk: "已完成",
      }),
    );
    await store.processStreamEvent(
      streamEvent({ type: "end", messageId: "assistant-1", generation: 7 }),
    );

    await store.processStreamEvent(
      streamEvent({
        type: "thinking",
        messageId: "assistant-1",
        generation: 7,
      }),
    );
    await store.processStreamEvent(
      streamEvent({
        type: "data",
        messageId: "assistant-1",
        generation: 7,
        chunk: "迟到正文",
      }),
    );
    await store.processStreamEvent(
      streamEvent({ type: "end", messageId: "assistant-1", generation: 7 }),
    );

    expect(
      store.getActiveStreamMessage("agent-a", "agent", "topic-a", "assistant-1")
        ?.content,
    ).toBe("已完成");
    expect(
      store.generationWatermarks.get("agent:agent-a:topic-a:assistant-1"),
    ).toMatchObject({ generation: 7 });
  });

  it("缺少或冲突的 generation/身份别名 fail-closed", () => {
    expect(
      parseStreamEvent({
        type: "thinking",
        messageId: "assistant-1",
        context: streamContext,
      }),
    ).toBeNull();
    expect(
      parseStreamEvent({
        type: "thinking",
        generation: 3,
        request_epoch: 4,
        messageId: "assistant-1",
        context: streamContext,
      }),
    ).toBeNull();
    expect(
      parseStreamEvent({
        type: "thinking",
        generation: 3,
        messageId: "assistant-1",
        message_id: "other-message",
        context: streamContext,
      }),
    ).toBeNull();
    expect(
      parseStreamEvent({
        type: "thinking",
        generation: null,
        messageId: "assistant-1",
        context: streamContext,
      }),
    ).toBeNull();
    expect(
      parseStreamEvent({
        type: "thinking",
        generation: "3",
        messageId: "assistant-1",
        context: streamContext,
      }),
    ).toBeNull();
    expect(
      parseStreamEvent({
        type: "thinking",
        generation: 3,
        messageId: "",
        requestId: "assistant-1",
        context: streamContext,
      }),
    ).toBeNull();
    expect(
      parseStreamEvent({
        type: "thinking",
        generation: 3,
        messageId: "assistant-1",
        context: { ...streamContext, ownerId: 17 },
      }),
    ).toBeNull();
  });

  it("owner 类型缺省可由单一 owner ID 推断，但交叉身份始终 fail-closed", () => {
    const baseEvent = {
      type: "thinking",
      generation: 1,
      messageId: "assistant-1",
    };
    expect(
      parseStreamEvent({
        ...baseEvent,
        context: { topicId: "topic-a", agentId: "agent-a" },
      }),
    ).toMatchObject({ identity: { ownerType: "agent", ownerId: "agent-a" } });
    expect(
      parseStreamEvent({
        ...baseEvent,
        context: { topicId: "topic-a", groupId: "group-a" },
      }),
    ).toMatchObject({ identity: { ownerType: "group", ownerId: "group-a" } });
    expect(
      parseStreamEvent({
        ...baseEvent,
        context: {
          topicId: "topic-a",
          groupId: "group-a",
          agentId: "agent-a",
        },
      }),
    ).toBeNull();
    expect(
      parseStreamEvent({
        ...baseEvent,
        context: { topicId: "topic-a", ownerType: "agent", groupId: "group-a" },
      }),
    ).toBeNull();
    expect(
      parseStreamEvent({
        ...baseEvent,
        context: { topicId: "topic-a", ownerType: "group", agentId: "agent-a" },
      }),
    ).toBeNull();
  });
});
