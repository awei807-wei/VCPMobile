import { describe, expect, it } from "vitest";
import chatManagerSource from "../../../../src-tauri/src/vcp_modules/chat/chat_manager.rs?raw";
import tauriLibSource from "../../../../src-tauri/src/lib.rs?raw";

describe("编辑重发 Tauri command 接线", () => {
  it("生产 wrapper 校验复合身份并复用 active request 取消语义", () => {
    expect(chatManagerSource).toMatch(
      /pub async fn edit_message_and_truncate_history\([\s\S]*?owner_id: String,[\s\S]*?owner_type: String,[\s\S]*?topic_id: String,[\s\S]*?anchor_message_id: String,[\s\S]*?message: ChatMessage,/,
    );
    expect(chatManagerSource).toMatch(
      /message_service::edit_message_and_truncate_history\(/,
    );
    expect(chatManagerSource).toMatch(/with_topic_mutation\(&topic_key/);
    expect(chatManagerSource).toMatch(
      /cancel_captured_active_request_epochs_all\(&active_registry, captured\)/,
    );
  });

  it("生产 invoke handler 导出前端使用的命令名称", () => {
    const occurrences =
      tauriLibSource.match(/edit_message_and_truncate_history/g) ?? [];
    expect(occurrences.length).toBeGreaterThanOrEqual(2);
    expect(tauriLibSource).toMatch(
      /generate_handler!\[[\s\S]*?edit_message_and_truncate_history[\s\S]*?\]/,
    );
  });
});
