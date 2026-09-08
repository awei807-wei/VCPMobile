import { invoke } from "@tauri-apps/api/core";

const FOREGROUND_EPOCH_KEY = "vcp.lifecycle.foreground_request_epoch";
const defaultScope = {};
const scopeEpochs = new WeakMap<object, number>();
// 时间戳只作为存储不可用时的正数起点，正常运行仍以持久化序号为准。
let processEpoch = Math.max(readStoredEpoch(), Date.now());

function readStoredEpoch(): number {
  if (typeof window === "undefined") return 0;
  try {
    const value = Number(window.localStorage.getItem(FOREGROUND_EPOCH_KEY));
    return Number.isSafeInteger(value) && value > 0 ? value : 0;
  } catch {
    return 0;
  }
}

function storeEpoch(epoch: number): void {
  if (typeof window === "undefined") return;
  try {
    const storage = window.localStorage;
    if (!storage) return;
    storage.setItem(FOREGROUND_EPOCH_KEY, String(epoch));
  } catch (error) {
    console.warn("[useAppLifecycle] 无法持久化前后台请求序号：", error);
  }
}

function nextEpoch(scope: object): number {
  const scopedEpoch = scopeEpochs.get(scope) ?? 0;
  const epoch = Math.max(scopedEpoch, processEpoch) + 1;
  scopeEpochs.set(scope, epoch);
  processEpoch = epoch;
  storeEpoch(epoch);
  return epoch;
}

/** 串行发送前后台变更，并使用跨挂载持续递增的后端请求序号。 */
export class ForegroundStateController {
  private tail = Promise.resolve();
  private disposed = false;

  constructor(private readonly scope: object = defaultScope) {}

  sync(isForeground: boolean): Promise<void> {
    const epoch = nextEpoch(this.scope);
    this.tail = this.tail
      .catch(() => undefined)
      .then(async () => {
        if (this.disposed) return;
        try {
          await invoke("set_app_foreground_state", {
            isForeground,
            requestEpoch: epoch,
          });
        } catch (error) {
          console.error("[useAppLifecycle] 同步前后台状态失败：", error);
        }
      });
    return this.tail;
  }

  dispose(): void {
    this.disposed = true;
  }
}
