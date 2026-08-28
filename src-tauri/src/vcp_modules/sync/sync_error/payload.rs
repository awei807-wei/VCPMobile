use super::registry::{error_definition, fallback_definition};
use super::types::{SyncErrorCategory, SyncErrorPayload, WireSyncError};
use super::validation::{is_valid_wire_code, sanitize_topic_ids};

fn fallback_copy(category: SyncErrorCategory) -> (&'static str, &'static str) {
    match category {
        SyncErrorCategory::Device => ("设备状态暂不满足同步条件", "按系统提示调整设备状态后再试。"),
        SyncErrorCategory::Configuration => {
            ("同步配置需要调整", "检查两端地址、端口和令牌后再试。")
        }
        SyncErrorCategory::Connection => (
            "同步通道中断或响应超时",
            "确认网络与电脑端服务正常后重新同步。",
        ),
        SyncErrorCategory::Compatibility => (
            "手机端与电脑端同步版本不兼容",
            "将两端更新到同一兼容版本后再试。",
        ),
        SyncErrorCategory::Protocol => (
            "同步响应不符合 Wire 1.2 规范，已安全停止",
            "确认两端版本一致并重启电脑端同步插件；若仍出现，请保留日志。",
        ),
        SyncErrorCategory::Data => (
            "部分同步数据缺失、冲突或无法处理",
            "检查电脑端对应数据后重新同步；若仍失败，请保留日志。",
        ),
        SyncErrorCategory::Storage => (
            "同步数据读取或写入失败",
            "检查两端存储与数据服务状态后重新同步；若仍失败，请保留日志。",
        ),
        SyncErrorCategory::Internal => (
            "同步组件未能正常完成本次任务",
            "重启应用后重新同步；若仍失败，请保留最新日志。",
        ),
    }
}

pub fn build_local_error_payload(
    code: &str,
    failed_topic_ids: Vec<String>,
    log_file: Option<String>,
) -> SyncErrorPayload {
    let stable_code = if is_valid_wire_code(code) {
        code
    } else {
        "SYNC_ATTEMPT_FAILED"
    };
    let selected = error_definition(stable_code).unwrap_or_else(fallback_definition);
    let (message, guidance) = fallback_copy(selected.category);
    SyncErrorPayload {
        code: stable_code.to_owned(),
        category: selected.category,
        origin: selected.origin,
        stage: selected.stage,
        retry_action: selected.retry,
        message: message.to_owned(),
        guidance: guidance.to_owned(),
        failed_topic_ids: sanitize_topic_ids(failed_topic_ids),
        log_file,
    }
}

pub fn build_wire_error_payload(
    wire: &WireSyncError,
    additional_failed_topic_ids: Vec<String>,
    log_file: Option<String>,
) -> SyncErrorPayload {
    let (message, guidance) = fallback_copy(wire.kind);
    let failed_topic_ids = wire
        .failed_topic_ids
        .iter()
        .cloned()
        .chain(additional_failed_topic_ids)
        .collect::<Vec<_>>();
    SyncErrorPayload {
        code: wire.code.clone(),
        category: wire.kind,
        origin: wire.origin,
        stage: wire.stage,
        retry_action: wire.retry,
        message: message.to_owned(),
        guidance: guidance.to_owned(),
        failed_topic_ids: sanitize_topic_ids(failed_topic_ids),
        log_file,
    }
}
