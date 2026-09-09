import { reactive, ref, type Ref } from "vue";
import type { ChatMessage } from "../types/chat";
import type {
  StreamState,
  UnreadMessageReceiptRecord,
  UnreadMessageRetryState,
} from "./chatStreamProcessorSupport";
import { createGenerationWatermarks } from "./chatStreamProcessorSupport";

export interface ChatStreamStoreState {
  streamingMessageId: Ref<string | null>;
  streamingMessageKey: Ref<string | null>;
  sessionActiveStreams: Ref<Record<string, string[]>>;
  pendingGenerationRequests: Ref<Record<string, string[]>>;
  activeStreamMessages: Map<string, ChatMessage>;
  rAFPendingUpdates: Map<string, any>;
  cleanupTimers: Set<ReturnType<typeof setTimeout>>;
  streamGenerations: Map<string, number>;
  generationWatermarks: Map<string, { generation: number }>;
  generationClocks: Map<string, number>;
  wireGenerations: Map<
    string,
    { wireGeneration: number; streamGeneration: number }
  >;
  unreadMessageKeys: Set<string>;
  unreadMessageInFlightKeys: Set<string>;
  unreadMessageRetryTimers: Map<string, ReturnType<typeof setTimeout>>;
  unreadMessageRetryStates: Map<string, UnreadMessageRetryState>;
  unreadMessageReceiptRecords: Map<string, UnreadMessageReceiptRecord>;
  unreadMessageFailedKeys: Set<string>;
  unreadMessageReceiptTombstones: Set<string>;
  unreadTopicReceiptTombstones: Set<string>;
  runtimeState: StreamState;
}

export function createChatStreamStoreState(): ChatStreamStoreState {
  const collections = createChatStreamStoreCollections();
  return {
    ...collections,
    runtimeState: createChatStreamRuntimeState(collections),
  };
}

type ChatStreamStoreCollections = Omit<ChatStreamStoreState, "runtimeState">;

function createChatStreamStoreCollections(): ChatStreamStoreCollections {
  const streamingMessageId = ref<string | null>(null);
  const streamingMessageKey = ref<string | null>(null);
  const sessionActiveStreams = ref<Record<string, string[]>>({});
  const pendingGenerationRequests = ref<Record<string, string[]>>({});
  const activeStreamMessages = reactive(new Map<string, ChatMessage>()) as Map<
    string,
    ChatMessage
  >;
  const unreadState = createChatStreamUnreadState();
  return {
    streamingMessageId,
    streamingMessageKey,
    sessionActiveStreams,
    pendingGenerationRequests,
    activeStreamMessages,
    rAFPendingUpdates: new Map<string, any>(),
    cleanupTimers: new Set<ReturnType<typeof setTimeout>>(),
    streamGenerations: new Map<string, number>(),
    generationWatermarks: createGenerationWatermarks(),
    generationClocks: new Map<string, number>(),
    wireGenerations: new Map(),
    ...unreadState,
  };
}

function createChatStreamUnreadState() {
  return {
    unreadMessageKeys: new Set<string>(),
    unreadMessageInFlightKeys: new Set<string>(),
    unreadMessageRetryTimers: new Map<string, ReturnType<typeof setTimeout>>(),
    unreadMessageRetryStates: new Map<string, UnreadMessageRetryState>(),
    unreadMessageReceiptRecords: new Map<string, UnreadMessageReceiptRecord>(),
    unreadMessageFailedKeys: new Set<string>(),
    unreadMessageReceiptTombstones: new Set<string>(),
    unreadTopicReceiptTombstones: new Set<string>(),
  };
}

function createChatStreamRuntimeState(
  collections: ChatStreamStoreCollections,
): StreamState {
  const {
    activeStreamMessages,
    rAFPendingUpdates,
    cleanupTimers,
    streamingMessageId,
    streamingMessageKey,
    streamGenerations,
    generationWatermarks,
    generationClocks,
    wireGenerations,
    unreadMessageKeys,
    unreadMessageInFlightKeys,
    unreadMessageRetryTimers,
    unreadMessageRetryStates,
    unreadMessageReceiptRecords,
    unreadMessageFailedKeys,
    unreadMessageReceiptTombstones,
    unreadTopicReceiptTombstones,
  } = collections;
  return {
    activeStreamMessages,
    rAFPendingUpdates,
    cleanupTimers,
    streamingMessageId,
    streamingMessageKey,
    streamGenerations,
    generationWatermarks,
    generationClocks,
    wireGenerations,
    unreadMessageKeys,
    unreadMessageInFlightKeys,
    unreadMessageRetryTimers,
    unreadMessageRetryStates,
    unreadMessageReceiptRecords,
    unreadMessageFailedKeys,
    unreadMessageReceiptTombstones,
    unreadTopicReceiptTombstones,
  };
}
