import { afterEach, describe, expect, it, vi } from "vitest";
import {
  getWebUploadTimeoutMs,
  MAX_WEB_UPLOAD_TIMEOUT_MS,
  MIN_WEB_UPLOAD_TIMEOUT_MS,
  uploadFileWithXhr,
} from "../../core/stores/attachmentUpload";

class FakeUpload {
  onprogress?: (event: ProgressEvent) => void;
}

class FakeXmlHttpRequest {
  static instances: FakeXmlHttpRequest[] = [];
  readonly upload = new FakeUpload();
  responseText = "";
  status = 0;
  timeout = 0;
  onload?: () => void;
  onerror?: () => void;
  ontimeout?: () => void;
  onabort?: () => void;
  open = vi.fn();
  setRequestHeader = vi.fn();
  send = vi.fn();

  constructor() {
    FakeXmlHttpRequest.instances.push(this);
  }
}

describe("Web 附件上传", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    FakeXmlHttpRequest.instances = [];
  });

  it("设置有界超时并解析有效 JSON", async () => {
    vi.stubGlobal("XMLHttpRequest", FakeXmlHttpRequest);
    const progress = vi.fn();
    const file = new Blob(["content"]);
    const resultPromise = uploadFileWithXhr(
      file,
      { url: "http://127.0.0.1/upload", token: "token" },
      progress,
    );
    const xhr = FakeXmlHttpRequest.instances[0];
    expect(xhr.timeout).toBeGreaterThanOrEqual(MIN_WEB_UPLOAD_TIMEOUT_MS);
    expect(xhr.timeout).toBeLessThanOrEqual(MAX_WEB_UPLOAD_TIMEOUT_MS);
    xhr.upload.onprogress?.({
      lengthComputable: true,
      loaded: 5,
      total: 10,
    } as ProgressEvent);
    xhr.status = 201;
    xhr.responseText = '{"hash":"abc"}';
    xhr.onload?.();

    await expect(resultPromise).resolves.toEqual({ hash: "abc" });
    expect(progress).toHaveBeenCalledWith({
      loaded: 5,
      total: 10,
      percent: 50,
    });
  });

  it.each([
    ["timeout", "上传超时"],
    ["abort", "上传已取消"],
  ])("XHR %s 时拒绝上传", async (event, message) => {
    vi.stubGlobal("XMLHttpRequest", FakeXmlHttpRequest);
    const resultPromise = uploadFileWithXhr(
      new Blob(["content"]),
      { url: "http://127.0.0.1/upload", token: "token" },
      () => undefined,
    );
    const xhr = FakeXmlHttpRequest.instances[0];
    xhr[event === "timeout" ? "ontimeout" : "onabort"]?.();
    await expect(resultPromise).rejects.toThrow(message);
  });

  it("拒绝格式错误的成功响应", async () => {
    vi.stubGlobal("XMLHttpRequest", FakeXmlHttpRequest);
    const resultPromise = uploadFileWithXhr(
      new Blob(["content"]),
      { url: "http://127.0.0.1/upload", token: "token" },
      () => undefined,
    );
    const xhr = FakeXmlHttpRequest.instances[0];
    xhr.status = 200;
    xhr.responseText = "not-json";
    xhr.onload?.();
    await expect(resultPromise).rejects.toThrow("无效响应");
  });

  it("超大文件的超时增长仍有上限", () => {
    expect(getWebUploadTimeoutMs(Number.MAX_SAFE_INTEGER)).toBe(
      MAX_WEB_UPLOAD_TIMEOUT_MS,
    );
  });
});
