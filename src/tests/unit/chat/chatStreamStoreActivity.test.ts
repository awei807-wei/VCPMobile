import { afterEach, describe, expect, it, vi } from "vitest";
import { createChatStreamActivity } from "../../../core/stores/chatStreamStoreActivity";
import { createChatStreamIdentityApi } from "../../../core/stores/chatStreamStoreIdentity";
import { createChatStreamStoreState } from "../../../core/stores/chatStreamStoreState";

const screenKeeper = vi.hoisted(() => ({
  releaseScreenKeep: vi.fn(),
}));

vi.mock("../../../core/composables/useScreenKeeper", () => ({
  releaseScreenKeep: screenKeeper.releaseScreenKeep,
}));

afterEach(() => {
  vi.useRealTimers();
  screenKeeper.releaseScreenKeep.mockReset();
});

describe("聊天流活动状态", () => {
  it("重复移除已结束的流时不会释放其他功能持有的亮屏引用", () => {
    vi.useFakeTimers();
    const identity = {
      ownerId: "owner-a",
      ownerType: "agent" as const,
      topicId: "topic-a",
    };
    const state = createChatStreamStoreState();
    const identityApi = createChatStreamIdentityApi({
      currentSelectedItem: { id: identity.ownerId, type: identity.ownerType },
      currentTopicId: identity.topicId,
    });
    const activity = createChatStreamActivity({ state, identity: identityApi });

    activity.addSessionStream(identity, "message-a");
    activity.removeSessionStream(identity, "message-a");
    activity.removeSessionStream(identity, "message-a");

    expect(screenKeeper.releaseScreenKeep).toHaveBeenCalledTimes(1);
  });
});
