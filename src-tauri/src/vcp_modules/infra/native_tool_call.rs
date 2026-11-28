//! 原生 Function Call（OpenAI `tool_calls`）分片组装器。
//!
//! 严格对齐 VCPChat 桌面端 `modules/chat/nativeToolCallStream.js`：
//! 服务端开启 `VCP_FC_FORWARD_TO_CLIENT` 后会把原生 `tool_calls` 原样转发，
//! 客户端只负责组装成展示协议块，**绝不重复执行工具**。

use serde_json::{json, Value};

use crate::vcp_modules::chat::hosted_tool_call::{HOSTED_TOOL_CALL_END, HOSTED_TOOL_CALL_START};

/// `tool_calls[].index` 上限，防止脏数据撑爆内存。
const MAX_TOOL_CALL_INDEX: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Default)]
struct Slot {
    index: usize,
    id: String,
    kind: String,
    name: String,
    arguments: String,
}

/// 按 `choice.index == 0` 与 `call.index` 合并分片的组装器。
#[derive(Debug, Default)]
pub(crate) struct NativeToolCallAssembler {
    slots: Vec<Slot>,
    invalid: bool,
}

/// `choices[0].finish_reason`，兼容部分代理把 `finish_reason` 放在顶层的旧行为。
pub(crate) fn choice_finish_reason(choice: &Value) -> Option<&str> {
    choice.get("finish_reason").and_then(Value::as_str)
}

fn call_index(call: &Value, position: usize) -> usize {
    call.get("index")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(position)
}

fn arguments_value(call: &Value) -> Option<&Value> {
    call.get("function")
        .and_then(|function| function.get("arguments"))
}

impl NativeToolCallAssembler {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn slot_mut(&mut self, index: usize) -> &mut Slot {
        if let Some(position) = self.slots.iter().position(|slot| slot.index == index) {
            return &mut self.slots[position];
        }
        self.slots.push(Slot {
            index,
            ..Slot::default()
        });
        self.slots.last_mut().expect("slot just pushed")
    }

    /// 吸收一帧 choice：完整态 `message.tool_calls` 直接赋值，`delta.tool_calls` 累加。
    pub(crate) fn ingest(&mut self, choice: &Value) {
        if let Some(calls) = choice
            .get("message")
            .and_then(|message| message.get("tool_calls"))
            .and_then(Value::as_array)
        {
            self.absorb_complete(calls);
            return;
        }
        if let Some(calls) = choice
            .get("delta")
            .and_then(|delta| delta.get("tool_calls"))
            .and_then(Value::as_array)
        {
            self.absorb_delta(calls);
        }
    }

    fn absorb_complete(&mut self, calls: &[Value]) {
        self.slots.clear();
        for (position, call) in calls.iter().enumerate() {
            let index = call_index(call, position);
            if index >= MAX_TOOL_CALL_INDEX {
                self.invalid = true;
                continue;
            }
            let slot = self.slot_mut(index);
            slot.id = call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            slot.kind = call
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            slot.name = call
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            slot.arguments = match arguments_value(call) {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            };
        }
    }

    fn absorb_delta(&mut self, calls: &[Value]) {
        for (position, call) in calls.iter().enumerate() {
            let index = call_index(call, position);
            if index >= MAX_TOOL_CALL_INDEX {
                self.invalid = true;
                continue;
            }
            let slot = self.slot_mut(index);
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                if !id.is_empty() && !slot.id.contains(id) {
                    slot.id.push_str(id);
                }
            }
            if let Some(kind) = call.get("type").and_then(Value::as_str) {
                if !kind.is_empty() {
                    slot.kind = kind.to_string();
                }
            }
            if let Some(name) = call
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
            {
                if !name.is_empty() {
                    slot.name.push_str(name);
                }
            }
            match arguments_value(call) {
                Some(Value::String(text)) => slot.arguments.push_str(text),
                Some(Value::Null) | None => {}
                Some(other) => slot.arguments.push_str(&other.to_string()),
            }
        }
    }

    /// 是否到达一批工具调用的硬边界。
    ///
    /// 1. 当前帧有非空正文且无 tool_calls（适配多轮工具调用）；
    /// 2. 收到完整态 `message.tool_calls`；
    /// 3. `finish_reason` 为 `tool_calls` / `stop`。
    pub(crate) fn should_flush(&self, choice: &Value, has_content: bool) -> bool {
        if self.slots.is_empty() {
            return false;
        }
        if has_content {
            return true;
        }
        if choice
            .get("message")
            .and_then(|message| message.get("tool_calls"))
            .is_some()
        {
            return true;
        }
        matches!(
            choice_finish_reason(choice),
            Some("tool_calls") | Some("stop")
        )
    }

    /// 校验并取走当前批次；任一校验失败则整批丢弃（宁可少展示，不可错展示）。
    fn take_calls(&mut self) -> Vec<NativeToolCall> {
        let mut slots = std::mem::take(&mut self.slots);
        let invalid = std::mem::take(&mut self.invalid);
        if invalid {
            return Vec::new();
        }
        slots.sort_by_key(|slot| slot.index);
        let mut seen = std::collections::HashSet::new();
        let mut calls = Vec::with_capacity(slots.len());
        for slot in slots {
            if slot.id.is_empty() || slot.name.is_empty() || slot.arguments.is_empty() {
                return Vec::new();
            }
            if !slot.kind.is_empty() && slot.kind != "function" {
                return Vec::new();
            }
            if !seen.insert(slot.id.clone()) {
                return Vec::new();
            }
            calls.push(NativeToolCall {
                id: slot.id,
                name: slot.name,
                arguments: slot.arguments,
            });
        }
        calls
    }

    /// 生成内嵌协议块文本；无有效调用时返回 None。
    pub(crate) fn take_marker(&mut self) -> Option<String> {
        let calls = self.take_calls();
        if calls.is_empty() {
            return None;
        }
        let mut marker = String::new();
        for call in calls {
            marker.push_str(HOSTED_TOOL_CALL_START);
            marker.push('\n');
            marker.push_str(
                &json!({
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": call.arguments,
                    }
                })
                .to_string(),
            );
            marker.push('\n');
            marker.push_str(HOSTED_TOOL_CALL_END);
            marker.push('\n');
        }
        Some(marker)
    }
}

#[cfg(test)]
mod tests {
    use super::NativeToolCallAssembler;
    use serde_json::json;

    #[test]
    fn delta_fragments_merge_by_call_index() {
        let mut assembler = NativeToolCallAssembler::new();
        assembler.ingest(&json!({
            "delta": {"tool_calls": [
                {"index": 0, "id": "call_a", "type": "function", "function": {"name": "vcptools", "arguments": "{\"tool_"}}
            ]}
        }));
        assembler.ingest(&json!({
            "delta": {"tool_calls": [
                {"index": 0, "function": {"arguments": "name\":\"GitSearch\"}"}}
            ]}
        }));
        let marker = assembler.take_marker().expect("marker");
        assert!(marker.contains("\"name\":\"vcptools\""));
        assert!(marker.contains("GitSearch"));
        assert!(marker.starts_with("<<<[HOSTED_TOOL_CALL]>>>"));
    }

    #[test]
    fn content_frame_flushes_pending_batch() {
        let mut assembler = NativeToolCallAssembler::new();
        assembler.ingest(&json!({
            "delta": {"tool_calls": [
                {"index": 0, "id": "call_a", "type": "function", "function": {"name": "GitSearch", "arguments": "{}"}}
            ]}
        }));
        assert!(assembler.should_flush(&json!({"delta": {"content": "后续正文"}}), true));
        assert!(assembler.take_marker().is_some());
        assert!(assembler.take_marker().is_none());
    }

    #[test]
    fn invalid_batch_is_dropped_instead_of_rendered() {
        let mut assembler = NativeToolCallAssembler::new();
        assembler.ingest(&json!({
            "delta": {"tool_calls": [
                {"index": 0, "id": "call_a", "type": "function", "function": {"name": "A", "arguments": "{}"}},
                {"index": 1, "id": "call_a", "type": "function", "function": {"name": "B", "arguments": "{}"}}
            ]}
        }));
        assert!(assembler.take_marker().is_none());
    }
}
