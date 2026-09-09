import { ref, type Ref } from "vue";

const SWIPE_THRESHOLD = 50;
const MAX_SWIPE = 80;
type SwipeDirection = "horizontal" | "vertical" | "undetermined";

interface SwipeState {
  activeSwipeId: Ref<string | null>;
  currentSwipeX: Ref<number>;
  isDragging: Ref<boolean>;
  startX: number;
  startY: number;
  isVerticalScroll: boolean;
  hasDeterminedDirection: boolean;
  isStartedAsSwiped: boolean;
}

const createSwipeState = (): SwipeState => ({
  activeSwipeId: ref(null),
  currentSwipeX: ref(0),
  isDragging: ref(false),
  startX: 0,
  startY: 0,
  isVerticalScroll: false,
  hasDeterminedDirection: false,
  isStartedAsSwiped: false,
});

export function ownerSwipeKey(type: "agent" | "group", id: string) {
  return `${type}:${id}`;
}

const closeActiveSwipe = (state: SwipeState) => {
  state.activeSwipeId.value = null;
  state.currentSwipeX.value = 0;
};

const handleTouchStart = (
  state: SwipeState,
  isSorting: Ref<boolean>,
  event: TouchEvent,
  id: string,
) => {
  if (isSorting.value) return;
  if (state.activeSwipeId.value && state.activeSwipeId.value !== id) {
    closeActiveSwipe(state);
  }
  state.startX = event.touches[0].clientX;
  state.startY = event.touches[0].clientY;
  state.isDragging.value = true;
  state.isVerticalScroll = false;
  state.isStartedAsSwiped = state.activeSwipeId.value === id;
  if (!state.isStartedAsSwiped) state.currentSwipeX.value = 0;
  state.hasDeterminedDirection = state.isStartedAsSwiped;
};

const resolveDirection = (
  state: SwipeState,
  deltaX: number,
  deltaY: number,
): SwipeDirection => {
  if (state.hasDeterminedDirection) {
    return state.isVerticalScroll ? "vertical" : "horizontal";
  }
  const absX = Math.abs(deltaX);
  const absY = Math.abs(deltaY);
  if (absX <= 3 && absY <= 3) return "undetermined";
  state.hasDeterminedDirection = true;
  if (absY / absX > 0.577) {
    state.isVerticalScroll = true;
    state.isDragging.value = false;
    return "vertical";
  }
  return "horizontal";
};

const updateCollapsedSwipe = (
  state: SwipeState,
  event: TouchEvent,
  id: string,
  deltaX: number,
) => {
  if (deltaX < 0) {
    state.isDragging.value = false;
    return;
  }
  if (event.cancelable) event.preventDefault();
  event.stopPropagation();
  state.activeSwipeId.value = id;
  state.currentSwipeX.value = Math.min(deltaX, MAX_SWIPE + 20);
};

const updateExpandedSwipe = (
  state: SwipeState,
  event: TouchEvent,
  deltaX: number,
) => {
  if (event.cancelable) event.preventDefault();
  event.stopPropagation();
  state.currentSwipeX.value =
    deltaX < 0
      ? Math.max(0, MAX_SWIPE + deltaX)
      : MAX_SWIPE + Math.min(deltaX, 20);
};

const handleTouchMove = (
  state: SwipeState,
  isSorting: Ref<boolean>,
  event: TouchEvent,
  id: string,
) => {
  if (isSorting.value || !state.isDragging.value || state.isVerticalScroll)
    return;
  const currentX = event.touches[0].clientX;
  const currentY = event.touches[0].clientY;
  const deltaX = currentX - state.startX;
  const deltaY = currentY - state.startY;
  if (resolveDirection(state, deltaX, deltaY) !== "horizontal") return;
  if (state.isStartedAsSwiped) {
    updateExpandedSwipe(state, event, deltaX);
  } else {
    updateCollapsedSwipe(state, event, id, deltaX);
  }
};

const handleTouchEnd = (
  state: SwipeState,
  isSorting: Ref<boolean>,
  event: TouchEvent,
  id: string,
) => {
  if (isSorting.value || !state.isDragging.value) return;
  state.isDragging.value = false;
  const shouldKeepOpen =
    state.activeSwipeId.value === id &&
    state.currentSwipeX.value > SWIPE_THRESHOLD;
  if (shouldKeepOpen) {
    state.currentSwipeX.value = MAX_SWIPE;
    event.stopPropagation();
    return;
  }
  closeActiveSwipe(state);
};

export function useAgentSwipe(isSorting: Ref<boolean>) {
  const state = createSwipeState();
  return {
    activeSwipeId: state.activeSwipeId,
    currentSwipeX: state.currentSwipeX,
    isDragging: state.isDragging,
    onTouchStart: (event: TouchEvent, id: string) =>
      handleTouchStart(state, isSorting, event, id),
    onTouchMove: (event: TouchEvent, id: string) =>
      handleTouchMove(state, isSorting, event, id),
    onTouchEnd: (event: TouchEvent, id: string) =>
      handleTouchEnd(state, isSorting, event, id),
    closeActiveSwipe: () => closeActiveSwipe(state),
  };
}
