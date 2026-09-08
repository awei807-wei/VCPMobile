<script setup lang="ts">
import { computed, watch, ref, nextTick, onUnmounted } from "vue";
import { useRouter } from "vue-router";
import { useVirtualList } from "@vueuse/core";
import { useTopicStore } from "../../core/stores/topicListManager";
import { useChatSessionStore } from "../../core/stores/chatSessionStore";
import { useLayoutStore } from "../../core/stores/layout";
import { useOverlayStore } from "../../core/stores/overlay";
import { useNotificationStore } from "../../core/stores/notification";
import {
  createTopicContextMenuItems,
  type TopicViewModel,
} from "./topicContextMenu";

const emit = defineEmits<{
  (e: "select-topic"): void;
}>();

const topicListStore = useTopicStore();
const sessionStore = useChatSessionStore();
const layoutStore = useLayoutStore();
const overlayStore = useOverlayStore();
const notificationStore = useNotificationStore();
const router = useRouter();

const currentTopics = computed<TopicViewModel[]>(() => {
  return topicListStore.filteredTopics as TopicViewModel[];
});

// 虚拟列表实现
const { list, containerProps, wrapperProps } = useVirtualList(currentTopics, {
  itemHeight: 74, // 10(h-10) + padding/margins, 约 74px
  overscan: 10,
});

// 拦截容器引用，用于数据变化时手动控制滚动位置
const scrollContainerRef = ref<HTMLElement | null>(null);
const bindContainerRef = (el: unknown) => {
  const htmlEl = el as HTMLElement | null;
  containerProps.ref.value = htmlEl;
  scrollContainerRef.value = htmlEl;
};

// 新建话题后自动滚动到顶部，强制虚拟列表重新计算并让用户看到新话题
watch(
  () => topicListStore.topics.length,
  async (newLen, oldLen) => {
    if (newLen > oldLen && scrollContainerRef.value) {
      await nextTick();
      scrollContainerRef.value.scrollTop = 0;
    }
  },
);

const sameTopic = (left: TopicViewModel, right: TopicViewModel) =>
  left.id === right.id &&
  left.ownerId === right.ownerId &&
  left.ownerType === right.ownerType;

const isCurrentTopic = (topic: TopicViewModel) =>
  sessionStore.currentTopicId === topic.id &&
  sessionStore.currentSelectedItem?.id === topic.ownerId &&
  sessionStore.currentSelectedItem?.type === topic.ownerType;

const showTopicContextMenu = (identity: TopicViewModel) => {
  // 每次打开菜单时，从 store 中获取最新的 topic 状态，避免闭包捕获旧状态
  const topic = topicListStore.topics.find((item) => sameTopic(item, identity));
  if (!topic) return;
  const menuItems = createTopicContextMenuItems(topic, {
    topicStore: topicListStore,
    overlayStore,
    notificationStore,
  });
  overlayStore.openContextMenu(menuItems, "Topic Options");
};

// 兜底同步：当聊天上下文的选中项变化时，自动重新加载对应 Agent/Group 的话题列表
watch(
  () =>
    [
      sessionStore.currentSelectedItem?.type,
      sessionStore.currentSelectedItem?.id,
    ] as const,
  ([ownerType, ownerId]) => {
    if ((ownerType === "agent" || ownerType === "group") && ownerId) {
      topicListStore.loadTopicList(ownerId, ownerType);
    }
  },
  { immediate: true },
);

const selectTopic = async (identity: TopicViewModel) => {
  if (router.currentRoute.value.path !== "/chat") {
    await router.push("/chat");
  }

  const topic = topicListStore.topics.find((candidate) =>
    sameTopic(candidate, identity),
  );
  if (!topic) {
    throw new Error(`Topic ${identity.id} has no complete owner identity`);
  }
  await sessionStore.selectTopicById(topic.ownerId, topic.ownerType, topic.id);

  // 顶部栏显示话题标题
  if (sessionStore.currentSelectedItem) {
    sessionStore.currentSelectedItem.name = topic.name;
  }

  // 在移动端，选择话题后自动关闭侧边栏
  layoutStore.setLeftDrawer(false);

  emit("select-topic");
};

// 话题搜索逻辑集成
const props = defineProps<{
  searchQuery?: string;
}>();

watch(
  () => props.searchQuery,
  (newVal) => {
    topicListStore.searchTerm = newVal || "";
  },
  { immediate: true },
);

onUnmounted(() => {
  topicListStore.searchTerm = "";
});
</script>

<template>
  <div
    v-if="!topicListStore.topics || topicListStore.topics.length === 0"
    class="p-8 opacity-30 text-center flex flex-col items-center gap-2"
  >
    <svg
      width="24"
      height="24"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
    >
      <path
        d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"
      ></path>
    </svg>
    <span class="text-xs">暂无话题，请先选择助手</span>
  </div>

  <div
    v-else
    :ref="bindContainerRef"
    :style="containerProps.style"
    @scroll="containerProps.onScroll"
    class="h-full overflow-y-auto vcp-scrollable px-4 py-4 no-rubber-band"
  >
    <div v-bind="wrapperProps" class="flex flex-col">
      <div
        v-for="item in list"
        :key="`${item.data.ownerType}:${item.data.ownerId}:${item.data.id}`"
        class="pb-2"
        @click="selectTopic(item.data)"
        v-longpress="() => showTopicContextMenu(item.data)"
      >
        <div
          class="relative p-3 glass-panel rounded-xl flex items-center gap-3 border shadow-sm cursor-pointer transition-[background-color,border-color,transform,box-shadow] duration-300 z-10 w-full active:scale-[0.98] origin-center"
          :class="[
            isCurrentTopic(item.data)
              ? 'glass-panel-active'
              : 'border-transparent hover:bg-black/5 dark:hover:bg-white/5',
          ]"
        >
          <!-- 未读小红点 / 计数角标 (基于桌面端主题同步) -->
          <div
            v-if="item.data.unreadCount && item.data.unreadCount > 0"
            class="absolute -top-1.5 -right-1.5 min-w-[18px] h-[18px] px-1 rounded-full border-2 border-white dark:border-gray-900 text-[9px] font-bold text-white flex items-center justify-center z-10 shadow-sm"
            style="
              background: linear-gradient(135deg, #ff6b6b 0%, #ee5a6f 100%);
            "
          >
            {{ item.data.unreadCount > 99 ? "99+" : item.data.unreadCount }}
          </div>
          <div
            v-else-if="item.data.unreadCount === -1 || item.data.unread"
            class="absolute -top-1 -right-1 w-3 h-3 rounded-full border-2 border-white dark:border-gray-900 z-10 shadow-sm shrink-0"
            style="background: #ff6b6b"
          ></div>

          <div
            class="relative w-10 h-10 rounded-xl flex items-center justify-center shrink-0 border border-black/5 dark:border-white/10"
            style="
              background: color-mix(
                in srgb,
                var(--highlight-text) 10%,
                transparent
              );
              color: var(--highlight-text);
            "
          >
            <svg
              width="18"
              height="18"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
            >
              <path
                d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"
              ></path>
            </svg>
          </div>
          <div class="flex flex-col overflow-hidden flex-1">
            <div class="flex justify-between items-center w-full">
              <span class="font-bold text-sm truncate text-primary-text">{{
                item.data.name
              }}</span>
              <span
                v-if="item.data.msgCount !== undefined"
                class="text-[11px] font-bold shrink-0 ml-2 px-[8px] py-[3px] rounded-[10px]"
                style="
                  background-color: var(--accent-bg);
                  color: var(--highlight-text);
                  font-family:
                    &quot;Arial Rounded MT Bold&quot;,
                    &quot;Helvetica Rounded&quot;, Arial, sans-serif;
                "
              >
                {{ item.data.msgCount }}
              </span>
            </div>
            <span
              class="text-[9px] text-secondary-text opacity-70 truncate font-mono tracking-tighter"
              >{{ item.data.id }}</span
            >
          </div>

          <!-- 解锁状态标签 (桌面端还原) -->
          <div
            v-if="!item.data.locked"
            class="absolute bottom-1 right-2 flex items-center gap-[2px] bg-black/5 dark:bg-white/10 px-1 py-[1px] rounded text-[9px] text-yellow-600 dark:text-yellow-400 border border-yellow-600/20 dark:border-yellow-400/20"
          >
            <LockOpen :size="8" />
            <span
              class="scale-90 font-bold uppercase tracking-tighter leading-none pt-[1px]"
              >Unlock</span
            >
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
