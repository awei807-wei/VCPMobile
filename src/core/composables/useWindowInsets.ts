import { onMounted, onUnmounted, ref, type Ref } from "vue";

interface NativeWindowInsetsDetail {
  systemTop?: number;
  systemBottom?: number;
  statusTop?: number;
  navigationBottom?: number;
  imeBottom?: number;
  keyboardVisible?: boolean;
}

interface KeyboardInsetDetail {
  height: number;
  visible: boolean;
  safeAreaBottom?: number;
}

interface UseWindowInsetsReturn {
  safeTop: Ref<number>;
  safeBottom: Ref<number>;
  navigationBottom: Ref<number>;
  keyboardBottom: Ref<number>;
}

const safeTop = ref(0);
const safeBottom = ref(0);
const navigationBottom = ref(0);
const keyboardBottom = ref(0);

let receivedWindowInsets = false;

const toCssPx = (physicalPx: unknown): number => {
  if (typeof physicalPx !== "number" || !Number.isFinite(physicalPx)) return 0;
  const dpr = window.devicePixelRatio || 1;
  return Math.max(0, Math.round(physicalPx / dpr));
};

const writeRootInsetVars = (detail: NativeWindowInsetsDetail) => {
  const root = document.documentElement;
  const top = toCssPx(detail.statusTop ?? detail.systemTop ?? 0);
  const navBottom = toCssPx(detail.navigationBottom ?? detail.systemBottom ?? 0);
  const imeBottom = toCssPx(detail.imeBottom ?? 0);
  const bottom = detail.keyboardVisible ? 0 : navBottom;

  safeTop.value = top;
  safeBottom.value = bottom;
  navigationBottom.value = navBottom;
  keyboardBottom.value = imeBottom;

  root.style.setProperty("--vcp-safe-top", `${top}px`);
  root.style.setProperty("--vcp-safe-bottom", `${bottom}px`);
  root.style.setProperty("--vcp-navigation-bottom", `${navBottom}px`);
  root.style.setProperty("--vcp-keyboard-bottom", `${imeBottom}px`);
};

/**
 * 同步 Android 原生 WindowInsets 到全局 CSS 变量。
 *
 * Android WebView 的 `env(safe-area-inset-bottom)` 在部分定制系统三键导航下会返回 0；
 * 原生层使用 `WindowInsetsCompat.Type.navigationBars()` 获取真实导航栏高度后，
 * 这里统一写入 `--vcp-safe-bottom`，让所有底部布局复用同一动态安全区。
 */
export function useWindowInsets(): UseWindowInsetsReturn {
  const handleWindowInsets = (e: Event) => {
    receivedWindowInsets = true;
    writeRootInsetVars((e as CustomEvent<NativeWindowInsetsDetail>).detail ?? {});
  };

  const handleKeyboardInsetsFallback = (e: Event) => {
    if (receivedWindowInsets) return;
    const detail = (e as CustomEvent<KeyboardInsetDetail>).detail;
    if (!detail) return;

    writeRootInsetVars({
      navigationBottom: detail.safeAreaBottom,
      imeBottom: detail.height,
      keyboardVisible: detail.visible,
    });
  };

  onMounted(() => {
    window.addEventListener("vcp-window-insets", handleWindowInsets);
    window.addEventListener("vcp-keyboard-inset", handleKeyboardInsetsFallback);
  });

  onUnmounted(() => {
    window.removeEventListener("vcp-window-insets", handleWindowInsets);
    window.removeEventListener("vcp-keyboard-inset", handleKeyboardInsetsFallback);
  });

  return {
    safeTop,
    safeBottom,
    navigationBottom,
    keyboardBottom,
  };
}
