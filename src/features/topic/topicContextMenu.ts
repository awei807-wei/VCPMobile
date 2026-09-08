import {
  CheckCircle,
  Copy,
  Edit3,
  Lock,
  LockOpen,
  Trash2,
} from "lucide-vue-next";
import type { OverlayActionItem } from "../../core/types/overlay";
import type { useNotificationStore } from "../../core/stores/notification";
import type { useOverlayStore } from "../../core/stores/overlay";
import type { useTopicStore } from "../../core/stores/topicListManager";
import type { Topic } from "../../core/stores/topicTypes";

export type TopicViewModel = Topic & { pinned?: boolean; updatedAt?: number };

type TopicStoreApi = Pick<
  ReturnType<typeof useTopicStore>,
  "updateTopicTitle" | "toggleTopicLock" | "setTopicUnread" | "deleteTopic"
>;
type OverlayStoreApi = Pick<
  ReturnType<typeof useOverlayStore>,
  "openPrompt" | "showConfirm"
>;
type NotificationStoreApi = Pick<
  ReturnType<typeof useNotificationStore>,
  "addNotification"
>;

export interface TopicContextMenuServices {
  topicStore: TopicStoreApi;
  overlayStore: OverlayStoreApi;
  notificationStore: NotificationStoreApi;
}

function createRenameTopicItem(
  topic: TopicViewModel,
  services: TopicContextMenuServices,
): OverlayActionItem {
  return {
    label: "修改标题",
    icon: Edit3,
    handler: () => {
      services.overlayStore.openPrompt({
        title: "修改话题标题",
        initialValue: topic.name,
        placeholder: "请输入新的话题标题...",
        onConfirm: (newTitle: string) => {
          if (newTitle && newTitle.trim()) {
            services.topicStore.updateTopicTitle(
              topic.ownerId,
              topic.ownerType,
              topic.id,
              newTitle.trim(),
            );
          }
        },
      });
    },
  };
}

function createCopyTopicIdItem(
  topic: TopicViewModel,
  notificationStore: NotificationStoreApi,
): OverlayActionItem {
  return {
    label: "复制 ID",
    icon: Copy,
    handler: async () => {
      try {
        await navigator.clipboard.writeText(topic.id);
        notificationStore.addNotification({
          type: "info",
          title: "复制成功",
          message: "话题 ID 已复制到剪贴板",
          toastOnly: true,
        });
      } catch (error) {
        console.error("Failed to copy ID:", error);
        notificationStore.addNotification({
          type: "error",
          title: "复制失败",
          message: "无法访问剪贴板",
          toastOnly: true,
        });
      }
    },
  };
}

function createAgentTopicItems(
  topic: TopicViewModel,
  topicStore: TopicStoreApi,
): OverlayActionItem[] {
  return [
    {
      label: topic.locked ? "解锁话题" : "锁定话题",
      icon: topic.locked ? LockOpen : Lock,
      handler: () =>
        topicStore.toggleTopicLock(topic.ownerId, topic.ownerType, topic.id),
    },
    {
      label: topic.unread ? "标为已读" : "标为未读",
      icon: CheckCircle,
      handler: () =>
        topicStore.setTopicUnread(
          topic.ownerId,
          topic.ownerType,
          topic.id,
          !topic.unread,
        ),
    },
  ];
}

function createDeleteTopicItem(
  topic: TopicViewModel,
  services: TopicContextMenuServices,
): OverlayActionItem {
  return {
    label: "删除话题",
    icon: Trash2,
    danger: true,
    handler: async () => {
      const confirmed = await services.overlayStore.showConfirm({
        title: "删除话题",
        message: `确定要删除话题“${topic.name}”吗？此操作不可撤销。`,
        confirmText: "继续",
        isDanger: true,
      });
      if (!confirmed) return;

      const finalConfirmed = await services.overlayStore.showConfirm({
        title: "最终确认",
        message: `永久删除话题“${topic.name}”及其聊天记录？`,
        confirmText: "永久删除",
        isDanger: true,
      });
      if (!finalConfirmed) return;

      try {
        await services.topicStore.deleteTopic(
          topic.ownerId,
          topic.ownerType,
          topic.id,
        );
      } catch (error) {
        console.error("[TopicList] Failed to delete topic:", error);
        services.notificationStore.addNotification({
          type: "error",
          title: "删除话题失败",
          message: "话题未被删除，请稍后重试。",
          toastOnly: true,
        });
      }
    },
  };
}

export function createTopicContextMenuItems(
  topic: TopicViewModel,
  services: TopicContextMenuServices,
): OverlayActionItem[] {
  const items = [
    createRenameTopicItem(topic, services),
    createCopyTopicIdItem(topic, services.notificationStore),
  ];
  if (topic.ownerType === "agent") {
    items.push(...createAgentTopicItems(topic, services.topicStore));
  }
  items.push(createDeleteTopicItem(topic, services));
  return items;
}
