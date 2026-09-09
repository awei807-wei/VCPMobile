import { beforeEach, describe, expect, it } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { mockInvoke } from "../mocks/tauri";
import {
  ownerUnreadKey,
  useAssistantStore,
} from "../../core/stores/assistant";

describe("owner unread aggregation key contract", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("matches Rust encodeURIComponent examples", () => {
    const cases = [
      ["owner!x", "owner!x"],
      ["owner with space", "owner%20with%20space"],
      ["owner%value", "owner%25value"],
      ["owner/path", "owner%2Fpath"],
      ["所有者/😀", "%E6%89%80%E6%9C%89%E8%80%85%2F%F0%9F%98%80"],
    ] as const;

    for (const [ownerId, encodedId] of cases) {
      expect(ownerUnreadKey("agent", ownerId)).toBe(`agent:${encodedId}`);
    }
  });

  it("keeps identical Agent and Group ids isolated in the aggregate", () => {
    const store = useAssistantStore();
    store.applyOwnerUnreadCount("same/id", "agent", 2);
    store.applyOwnerUnreadCount("same/id", "group", 5);

    expect(store.getOwnerUnreadCount("same/id", "agent")).toBe(2);
    expect(store.getOwnerUnreadCount("same/id", "group")).toBe(5);
    expect(store.unreadCounts[ownerUnreadKey("agent", "same/id")]).toBe(2);
    expect(store.unreadCounts[ownerUnreadKey("group", "same/id")]).toBe(5);
  });

  it("does not let a real-time Agent update overwrite a Group composite key", () => {
    const store = useAssistantStore();
    const groupId = "x/y";
    const agentId = "group:x%2Fy";

    store.applyOwnerUnreadCount(agentId, "agent", 11);
    store.applyOwnerUnreadCount(groupId, "group", 7);

    expect(store.getOwnerUnreadCount(agentId, "agent")).toBe(11);
    expect(store.getOwnerUnreadCount(groupId, "group")).toBe(7);
    expect(Object.keys(store.unreadCounts).sort()).toEqual([
      ownerUnreadKey("agent", agentId),
      ownerUnreadKey("group", groupId),
    ].sort());
  });

  it("loads snapshot counts without crossing Agent and Group encoded keys", async () => {
    const groupId = "x/y";
    const agentId = "group:x%2Fy";
    mockInvoke("get_assistants_snapshot", () => ({
      agents: [{ id: agentId, name: "Agent", model: "model" }],
      groups: [{ id: groupId, name: "Group", members: [] }],
      unreadCounts: {
        [ownerUnreadKey("agent", agentId)]: 11,
        [ownerUnreadKey("group", groupId)]: 7,
      },
    }));
    const store = useAssistantStore();

    await store.fetchAgentsAndGroups();

    expect(store.getOwnerUnreadCount(agentId, "agent")).toBe(11);
    expect(store.getOwnerUnreadCount(groupId, "group")).toBe(7);
    expect(Object.keys(store.unreadCounts).sort()).toEqual([
      ownerUnreadKey("agent", agentId),
      ownerUnreadKey("group", groupId),
    ].sort());
  });
});
