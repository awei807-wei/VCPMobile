export interface VcpUploadEndpoint {
  url: string;
  token: string;
}

export interface UploadProgress {
  loaded: number;
  total: number;
  percent: number;
}

export const MIN_WEB_UPLOAD_TIMEOUT_MS = 30_000;
export const MAX_WEB_UPLOAD_TIMEOUT_MS = 5 * 60 * 1_000;
const ESTIMATED_BYTES_PER_SECOND = 256 * 1_024;

/** 限制本地 TCP 交接等待时间，同时避免小文件承担不必要的长超时。 */
export function getWebUploadTimeoutMs(size: number): number {
  const safeSize = Number.isFinite(size) && size > 0 ? size : 0;
  const estimatedMs =
    MIN_WEB_UPLOAD_TIMEOUT_MS +
    Math.ceil(safeSize / ESTIMATED_BYTES_PER_SECOND) * 1_000;
  return Math.min(MAX_WEB_UPLOAD_TIMEOUT_MS, estimatedMs);
}

function parseUploadResponse(responseText: string): Record<string, unknown> {
  if (!responseText.trim()) {
    throw new Error("上传服务返回空响应");
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(responseText);
  } catch {
    throw new Error("上传服务返回了无效响应");
  }

  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("上传服务返回格式错误");
  }
  return parsed as Record<string, unknown>;
}

function createUploadError(message: string, cause?: unknown): Error {
  const error = new Error(message);
  if (cause !== undefined) {
    (error as Error & { cause?: unknown }).cause = cause;
  }
  return error;
}

function configureUploadRequest(
  xhr: XMLHttpRequest,
  file: Blob,
  endpoint: VcpUploadEndpoint,
  onProgress: (progress: UploadProgress) => void,
  resolveOnce: (value: Record<string, unknown>) => void,
  rejectOnce: (error: Error) => void,
) {
  xhr.open("POST", endpoint.url, true);
  xhr.timeout = getWebUploadTimeoutMs(file.size);
  xhr.setRequestHeader("Content-Type", "application/octet-stream");
  xhr.setRequestHeader("X-Upload-Token", endpoint.token);
  xhr.upload.onprogress = (event) => {
    if (!event.lengthComputable || event.total <= 0) return;
    onProgress({
      loaded: event.loaded,
      total: event.total,
      percent: Math.round((event.loaded / event.total) * 100),
    });
  };
  xhr.onload = () => {
    if (xhr.status < 200 || xhr.status >= 300) {
      rejectOnce(createUploadError(`上传失败（HTTP ${xhr.status}）`));
      return;
    }
    try {
      resolveOnce(parseUploadResponse(xhr.responseText));
    } catch (error) {
      rejectOnce(
        error instanceof Error
          ? error
          : createUploadError("上传服务返回了无效响应", error),
      );
    }
  };
  xhr.onerror = () => rejectOnce(createUploadError("上传网络连接失败"));
  xhr.ontimeout = () =>
    rejectOnce(createUploadError("上传超时，请检查网络后重试"));
  xhr.onabort = () => rejectOnce(createUploadError("上传已取消"));
}

/** 上传浏览器文件，并确保每条 XHR 终止路径只结算一次。 */
export function uploadFileWithXhr(
  file: Blob,
  endpoint: VcpUploadEndpoint,
  onProgress: (progress: UploadProgress) => void,
): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    let settled = false;

    const resolveOnce = (value: Record<string, unknown>) => {
      if (settled) return;
      settled = true;
      resolve(value);
    };
    const rejectOnce = (error: Error) => {
      if (settled) return;
      settled = true;
      reject(error);
    };

    try {
      configureUploadRequest(
        xhr,
        file,
        endpoint,
        onProgress,
        resolveOnce,
        rejectOnce,
      );
      xhr.send(file);
    } catch (error) {
      rejectOnce(createUploadError("启动上传失败", error));
    }
  });
}
