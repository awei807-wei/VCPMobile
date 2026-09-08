// @vitest-environment happy-dom

import { describe, expect, it, vi } from "vitest";
import { createStreamEventProcessor } from "../../../core/stores/chatStreamProcessor";
import type { StreamProcessorDeps } from "../../../core/stores/chatStreamProcessorSupport";

function createDeps(): StreamProcessorDeps {
  return {
    state: {
      activeStreamMessages: new Map(),
      rAFPendingUpdates: new Map(),
      cleanupTimers: new Set(),
      streamingMessageId: { value: null },
      streamingMessageKey: { value: null },
      streamGenerations: new Map(),
      generationWatermarks: new Map(),
    },
    computeShell: () => undefined,
    addSessionStream: vi.fn(),
    removeSessionStream: vi.fn(),
    incrementTopicMsgCount: vi.fn(),
    incrementTopicUnreadCount: vi.fn(),
    currentIdentity: () => null,
    invoke: vi.fn(async () => []),
    isStreamDebugEnabled: () => false,
    recordStreamTrace: vi.fn(),
    streamDebugLog: vi.fn(),
  };
}

function event(
  type: string,
  generation: number,
  chunk?: string,
  messageId = "message-1",
) {
  return {
    type,
    generation,
    messageId,
    context: {
      ownerType: "agent",
      agentId: "agent-1",
      topicId: "topic-1",
    },
    ...(chunk === undefined ? {} : { chunk }),
  };
}

describe("聊天流并发终态保护", () => {
  it("thinking 和 end 只更新界面，不追加持久化骨架", async () => {
    const deps = createDeps();
    const process = createStreamEventProcessor(deps);

    await process(event("thinking", 1));
    await process(event("end", 1));

    expect(deps.invoke).not.toHaveBeenCalledWith(
      "append_single_message",
      expect.anything(),
    );
    expect(deps.state.activeStreamMessages.size).toBe(1);
  });

  it("新请求建立后，迟到的旧请求不能覆盖正文", async () => {
    const deps = createDeps();
    const process = createStreamEventProcessor(deps);

    await process(event("data", 1, "旧正文"));
    await process(event("thinking", 2));
    await process(event("data", 2, "新正文"));
    await process(event("data", 1, "迟到正文"));

    const message = deps.state.activeStreamMessages.get(
      "agent:agent-1:topic-1:message-1",
    );
    expect(message?.content).toBe("新正文");
  });

  it("单流终结后，迟到旧终态不会重新创建活动消息", async () => {
    const deps = createDeps();
    const process = createStreamEventProcessor(deps);
    const created = vi.fn();

    await process(event("data", 2, "正文"), { onMessageCreated: created });
    await process(event("end", 2), { onMessageCreated: created });
    await process(event("thinking", 1), { onMessageCreated: created });

    expect(created).toHaveBeenCalledTimes(1);
    expect(
      deps.state.activeStreamMessages.get("agent:agent-1:topic-1:message-1")
        ?.content,
    ).toBe("正文");
  });

  it("首次 await 前关闭 terminal，gen2/gen3 反序完成仍保持最高水位", async () => {
    const deps = createDeps();
    const gates = [
      deferred<unknown>(),
      deferred<unknown>(),
      deferred<unknown>(),
    ];
    let compileIndex = 0;
    deps.invoke = vi.fn(async () => {
      const gate = gates[compileIndex++];
      gate.entered.resolve();
      return gate.result.promise;
    });
    const process = createStreamEventProcessor(deps);

    const terminal1 = process(event("end", 1));
    await gates[0].entered.promise;
    await process(event("data", 1, "迟到"));

    await process(event("thinking", 2));
    await process(event("data", 2, "二"));
    const terminal2 = process(event("end", 2));
    await gates[1].entered.promise;

    await process(event("thinking", 3));
    await process(event("data", 3, "三"));
    const terminal3 = process(event("end", 3));
    await gates[2].entered.promise;

    gates[2].result.resolve([]);
    await terminal3;
    gates[1].result.resolve([]);
    await terminal2;
    gates[0].result.resolve([]);
    await terminal1;

    expect(
      deps.state.activeStreamMessages.get("agent:agent-1:topic-1:message-1")
        ?.content,
    ).toBe("三");
    expect(
      deps.state.generationWatermarks?.get("agent:agent-1:topic-1:message-1")
        ?.generation,
    ).toBe(3);
    expect(deps.state.streamGenerations.size).toBe(0);
  });

  it("257 个终态与时间推进后仍拒绝已关闭 generation 重开", async () => {
    vi.useFakeTimers();
    try {
      const deps = createDeps();
      const process = createStreamEventProcessor(deps);
      await process(event("end", 1, undefined, "message-1"));
      for (let index = 2; index <= 257; index += 1)
        await process(event("end", 1, undefined, `message-${index}`));
      vi.advanceTimersByTime(60_001);
      await process(event("data", 1, "迟到", "message-1"));

      expect(
        deps.state.activeStreamMessages.get("agent:agent-1:topic-1:message-1")
          ?.content,
      ).toBe("");
      expect(
        deps.state.generationWatermarks?.get("agent:agent-1:topic-1:message-1")
          ?.generation,
      ).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });
});

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const result = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  let enter!: () => void;
  const entered = new Promise<void>((resolveEntered) => {
    enter = resolveEntered;
  });
  return {
    result: { promise: result, resolve },
    entered: { promise: entered, resolve: enter },
  };
}
