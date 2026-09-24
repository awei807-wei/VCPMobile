use crate::vcp_modules::chat_manager::{Attachment, ChatMessage};
use serde_json::{json, Value};
use sqlx::{Pool, Sqlite};

// =================================================================
// vcp_modules/chat/context_assembler.rs - 上下文级联装配中枢
// =================================================================
// 本模块承载了整个 VCP 大模型会话上下文注入的核心生命周期：
// 1. 【微观编织阶段】(assemble_history_for_vcp)：逐条迭代 SQLite 强类型 ChatMessage，
//    将分钟级时间戳 (带 \n 物理 Token 防火墙) 以及发言人前缀消歧编织进每条消息正文。
// 2. 【宏观拦截阶段】(apply_tarven_pipeline)：针对已序列化好的 messages 列表，
//    进行 System Metadata 环境真理注入、System/User Tavern 规则终极前后拼接拼接与虚拟节点插入。
// 3. 【统一装配外观】(orchestrate_chat_context)：向单聊与群聊业务模块提供极度纯净的 Facade 入口。

/// 统一上下文级联装配外观入口 (Facade Orchestrator)
pub struct ChatContextRequest<'a> {
    pub pool: &'a Pool<Sqlite>,
    pub history: &'a [ChatMessage],
    pub owner_id: &'a str,
    pub topic_id: &'a str,
    pub agent_name: &'a str,
    pub scope: &'a str, // "agent" | "group"
    pub base_system_prompt: String,
    pub invite_prompt: Option<String>,
}

pub async fn orchestrate_chat_context(
    request: ChatContextRequest<'_>,
) -> Result<Vec<Value>, String> {
    let ChatContextRequest {
        pool,
        history,
        owner_id,
        topic_id,
        agent_name,
        scope,
        base_system_prompt,
        invite_prompt,
    } = request;
    let enable_time_anchoring = load_time_anchoring_enabled(pool).await;

    // 2. 第一阶段：微观编织。进行强类型的发言人前缀及物理 Token 换行符时间隔离注入
    let is_group = scope == "group";
    let mut messages = assemble_history_for_vcp(history, is_group, enable_time_anchoring);

    // 3. 如果是群聊且存在主动邀请词 (Invite Prompt)，将其作为最新一轮用户消息拼装，以接受后续 Tavern 规则注入
    append_invite_prompt(&mut messages, invite_prompt);

    // 4. 将基础的 System Prompt 注入 Payload 首部
    prepend_system_prompt(&mut messages, base_system_prompt);

    // 5. 第二阶段：宏观拦截。调用 Tavern 拦截器流水线进行环境真理及 System/User 规则的终极拼装
    crate::vcp_modules::chat::context_injection::apply_tarven_pipeline(
        pool,
        owner_id,
        topic_id,
        agent_name,
        scope,
        &mut messages,
    )
    .await?;

    Ok(messages)
}

async fn load_time_anchoring_enabled(pool: &Pool<Sqlite>) -> bool {
    match sqlx::query_scalar::<_, i32>(
        "SELECT is_enabled FROM tarven_rules WHERE id = 'time_anchoring_v2'",
    )
    .fetch_optional(pool)
    .await
    {
        Ok(Some(value)) => value != 0,
        _ => false,
    }
}

fn append_invite_prompt(messages: &mut Vec<Value>, invite_prompt: Option<String>) {
    if let Some(invite) = invite_prompt.filter(|invite| !invite.is_empty()) {
        messages.push(json!({
            "role": "user",
            "content": invite
        }));
    }
}

fn prepend_system_prompt(messages: &mut Vec<Value>, base_system_prompt: String) {
    if !base_system_prompt.is_empty() {
        messages.insert(
            0,
            json!({
                "role": "system",
                "content": base_system_prompt
            }),
        );
    }
}

fn attachment_path(attachment: &Attachment) -> &str {
    if !attachment.internal_path.is_empty() {
        &attachment.internal_path
    } else {
        &attachment.src
    }
}

fn sanitized_file_name(candidate: &str) -> Option<String> {
    let base_name = candidate
        .rsplit(|character| character == '/' || character == '\\')
        .next()
        .unwrap_or(candidate);
    let cleaned = base_name
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || matches!(cleaned, "." | "..") {
        None
    } else {
        Some(cleaned.to_string())
    }
}

fn attachment_display_name(attachment: &Attachment, path: &str) -> String {
    sanitized_file_name(&attachment.name)
        .or_else(|| sanitized_file_name(path))
        .unwrap_or_else(|| "附件".to_string())
}

/// =================================================================
/// 🌌 微观历史记录编织器 (assemble_history_for_vcp)
/// =================================================================
/// 该函数负责把扁平的、面向 SQLite 的强类型 ChatMessage 关系数据结构，
/// 降维并映射为符合大模型 (LLM) Chat Completion API 规范的多模态 JSON Payload。
///
/// 🛡️ 双重换行物理防火墙 (BPE Token Barrier) 设计：
/// -------------------------------------------------------------
/// 为了防范 LLM 的 BPE 分词器 (Tokenizer) 将 "元数据前缀" 与 "消息正文" 的首个单词
/// 强行融合成单个不可预知的 Token，从而导致指示词语义降级甚至产生幻觉，
/// 我们在 "时间元数据"、"发言人消歧元数据" 与 "消息内容正文" 之间，
/// 强行硬编码级联插入了物理换行符 `\n`。这在字节层面上彻底切断了前缀与正文的融合通道。
///
/// 格式示意：
/// [Time: 2026-05-30 11:30]\n       <--- 物理换行 1：阻断时间与发言人特征融合
/// [Sender的发言]:\n                <--- 物理换行 2：阻断发言人与正文特征融合
/// 这是正文内容...
///
/// 📂 附件/多模态与内联物理隔离逻辑：
/// -------------------------------------------------------------
/// 1. 【文档类提取】：若附件（如 PDF、DOCX、TXT 等）已被 Rust 底层流水线提取为文本 `extracted_text`，
///    将以极其工整的形式通过内联闭环标签嵌入到文本尾部：
///    `\n\n[附加文件: {name}]\n{text}\n[/附加文件结束: {name}]`
/// 2. 【多模态富资产】：图片、音频或视频资产仅编译为带 MIME、本地路径与安全文件名的 `local_file`
///    标准 JSON 对象。网络请求预处理成功时只发送真实多模态节点；只有转换失败时才由下游生成纯文本降级提示。
pub fn assemble_history_for_vcp(
    history: &[ChatMessage],
    is_group: bool,
    enable_time_anchoring: bool,
) -> Vec<Value> {
    let mut result = Vec::new();

    for msg in history
        .iter()
        .filter(|msg| !msg.is_thinking.unwrap_or(false))
    {
        use chrono::TimeZone;
        let formatted_time = if let Some(dt) = chrono::Local
            .timestamp_millis_opt(msg.timestamp as i64)
            .single()
        {
            dt.format("%Y-%m-%d %H:%M").to_string()
        } else {
            chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
        };

        let mut combined_text = String::new();

        // 2. 发言人消歧前缀 (元数据 B) + 物理换行 2
        if is_group {
            let speaker_name = msg
                .name
                .as_ref()
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or_else(|| {
                    if msg.role == "user" {
                        "User".to_string()
                    } else {
                        "AI".to_string()
                    }
                });
            combined_text.push_str(&format!("[{}的发言]:\n", speaker_name));
        }

        // 3. 核心消息正文
        combined_text.push_str(&msg.content);

        let mut content_parts = Vec::new();

        if let Some(attachments) = &msg.attachments {
            for att in attachments {
                let path = attachment_path(att);
                let display_name = attachment_display_name(att, path);

                // 1. 处理提取的文本内容 (文档类)
                if let Some(text) = &att.extracted_text {
                    if !text.is_empty() {
                        combined_text.push_str(&format!(
                            "\n\n[附加文件: {}]\n{}\n[/附加文件结束: {}]",
                            display_name, text, display_name
                        ));
                    }
                }

                // 2. 处理多模态文件 (图片/音频/视频)。成功路径只发送 local_file，
                // 不把本地路径或附件占位符混入模型正文。
                let mime = &att.r#type;
                let normalized_mime = mime.to_ascii_lowercase();
                let is_image = normalized_mime == "image" || normalized_mime.starts_with("image/");
                let is_audio = normalized_mime == "audio" || normalized_mime.starts_with("audio/");
                let is_video = normalized_mime == "video" || normalized_mime.starts_with("video/");

                if is_image || is_audio || is_video {
                    content_parts.push(json!({
                        "type": "local_file",
                        "path": path,
                        "mime": mime,
                        "name": display_name
                    }));
                } else if att.extracted_text.is_none() {
                    combined_text.push_str(&format!("\n\n[附加文件: {}]", display_name));
                }
            }
        }

        // 4. 新版时间锚定机制 (元数据 A - 伪系统/user内联块格式)
        if enable_time_anchoring && msg.role == "user" {
            // 对于 user 消息块，直接在内部注入
            let username = msg
                .name
                .as_deref()
                .filter(|n| !n.is_empty())
                .unwrap_or("User");
            combined_text.push_str(&format!(
                "\n<system_meta>[系统提示]：{}发送于{}.</system_meta>",
                username, formatted_time
            ));
        }

        if !combined_text.trim().is_empty() {
            content_parts.insert(
                0,
                json!({
                    "type": "text",
                    "text": combined_text
                }),
            );
        }

        let final_content = if content_parts.len() == 1 && content_parts[0]["type"] == "text" {
            content_parts[0]["text"].clone()
        } else {
            json!(content_parts)
        };

        let mut val = json!({
            "role": msg.role,
            "name": msg.name,
            "content": final_content
        });
        if !msg.id.is_empty() {
            val["__vcpchatTimestampMeta"] = json!({
                "messageId": msg.id,
                "role": msg.role,
                "timestamp": msg.timestamp,
                "contentHash": msg.content_hash
            });
        }

        // 将当前消息推入结果列表
        result.push(val);

        // 如果是 非user 消息且启用了时间锚定，在后面追加一个伪系统 user 块
        if enable_time_anchoring && msg.role != "user" {
            let agent_name = msg
                .name
                .as_deref()
                .filter(|n| !n.is_empty())
                .unwrap_or("AI");
            let pseudo_user_msg = json!({
                "role": "user",
                "content": format!(
                    "<system_meta>[系统提示]：上条消息由{}发送于{}.</system_meta>",
                    agent_name, formatted_time
                )
            });
            result.push(pseudo_user_msg);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::{assemble_history_for_vcp, Attachment, ChatMessage};

    fn attachment(mime: &str, path: &str, name: &str) -> Attachment {
        Attachment {
            r#type: mime.to_string(),
            internal_path: path.to_string(),
            name: name.to_string(),
            ..Default::default()
        }
    }

    fn user_message(content: &str, attachments: Vec<Attachment>) -> ChatMessage {
        ChatMessage {
            id: "message-1".to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            timestamp: 1_700_000_000_000,
            attachments: Some(attachments),
            ..Default::default()
        }
    }

    #[test]
    fn text_and_image_emit_text_plus_local_file_without_visible_placeholder() {
        let image_path = "/private/app/cache/photo.png";
        let history = vec![user_message(
            "请看这张图",
            vec![attachment("image/png", image_path, "photo.png")],
        )];

        let messages = assemble_history_for_vcp(&history, false, false);
        let parts = messages[0]["content"].as_array().expect("content parts");
        let text = parts[0]["text"].as_str().expect("text part");

        assert_eq!(parts.len(), 2);
        assert_eq!(text, "请看这张图");
        assert!(!text.contains("[附加图片"));
        assert!(!text.contains(image_path));
        assert_eq!(parts[1]["type"], "local_file");
        assert_eq!(parts[1]["path"], image_path);
        assert_eq!(parts[1]["mime"], "image/png");
        assert_eq!(parts[1]["name"], "photo.png");
    }

    #[test]
    fn image_only_message_does_not_create_empty_text_part() {
        let history = vec![user_message(
            "",
            vec![attachment("image", "/private/app/photo", "photo.png")],
        )];

        let messages = assemble_history_for_vcp(&history, false, false);
        let parts = messages[0]["content"].as_array().expect("content parts");

        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "local_file");
        assert_eq!(parts[0]["mime"], "image");
    }

    #[test]
    fn multiple_images_keep_attachment_order_and_accept_both_image_mime_forms() {
        let history = vec![user_message(
            "",
            vec![
                attachment("image", "/private/app/first", "first.png"),
                attachment("image/png", "/private/app/second.png", "second.png"),
            ],
        )];

        let messages = assemble_history_for_vcp(&history, false, false);
        let parts = messages[0]["content"].as_array().expect("content parts");

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["name"], "first.png");
        assert_eq!(parts[0]["mime"], "image");
        assert_eq!(parts[1]["name"], "second.png");
        assert_eq!(parts[1]["mime"], "image/png");
    }

    #[test]
    fn group_image_message_keeps_sender_prefix_without_image_placeholder() {
        let image_path = "/private/group/photo.png";
        let mut message = user_message("", vec![attachment("image/png", image_path, "photo.png")]);
        message.name = Some("Alice".to_string());

        let messages = assemble_history_for_vcp(&[message], true, false);
        let parts = messages[0]["content"].as_array().expect("content parts");
        let text = parts[0]["text"].as_str().expect("sender prefix");

        assert_eq!(parts.len(), 2);
        assert_eq!(text, "[Alice的发言]:\n");
        assert!(!text.contains("[附加图片"));
        assert!(!text.contains(image_path));
        assert_eq!(parts[1]["type"], "local_file");
    }

    #[test]
    fn extracted_document_text_uses_safe_file_name_in_markers() {
        let mut document = attachment(
            "application/pdf",
            "/private/app/documents/report.pdf",
            r"C:\Users\Alice\Documents\report.pdf",
        );
        document.extracted_text = Some("document body".to_string());
        let history = vec![user_message("请总结", vec![document])];

        let messages = assemble_history_for_vcp(&history, false, false);
        let text = messages[0]["content"].as_str().expect("text content");

        assert!(text.contains("[附加文件: report.pdf]\ndocument body\n[/附加文件结束: report.pdf]"));
        assert!(!text.contains("/private/app/documents"));
        assert!(!text.contains(r"C:\Users\Alice"));
    }

    #[test]
    fn ordinary_file_placeholder_uses_only_safe_file_name() {
        let history = vec![user_message(
            "附件如下",
            vec![attachment(
                "application/octet-stream",
                "/private/app/files/archive.zip",
                "",
            )],
        )];

        let messages = assemble_history_for_vcp(&history, false, false);
        let text = messages[0]["content"].as_str().expect("text content");

        assert!(text.contains("[附加文件: archive.zip]"));
        assert!(!text.contains("/private/app/files"));
    }
}
