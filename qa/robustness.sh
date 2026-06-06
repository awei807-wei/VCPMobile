#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

ROUNDS="${ROUNDS:-2}"
RUN_ANDROID="${RUN_ANDROID:-0}"

log() {
  printf '\n==> %s\n' "$1"
}

run() {
  printf '+ %s\n' "$*"
  "$@"
}

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

require_rg_count() {
  local expected="$1"
  local pattern="$2"
  shift 2
  local count
  count="$(rg -n "$pattern" "$@" | wc -l | tr -d ' ')"
  if [[ "$count" != "$expected" ]]; then
    fail "模式数量异常: $pattern 期望=$expected 实际=$count"
  fi
}

log "鲁棒性静态哨兵"
for script in qa/regression.sh qa/robustness.sh; do
  if git check-ignore -q "$script"; then
    fail "QA shell scripts must be trackable by Git: $script"
  fi
done
require_rg_count 1 "export function findAgentMessagePayload" src
if rg -q "function findAgentMessageToolPayload" src; then
  fail "AgentMessage 工具 payload 查找不应重新拆出平行递归 helper"
fi
rg -q "agentNotificationDedupLock" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt \
  || fail "Android AgentMessage 去重锁缺失"
rg -q "AtomicInteger" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt \
  || fail "Android AgentMessage 通知 ID 原子计数器缺失"
rg -q "nextAgentMessageNotificationId" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt \
  || fail "Android AgentMessage 通知 ID 生成函数缺失"
rg -q "showSystemNotification posted id=.*bodyLength=" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt \
  || fail "Android 系统通知日志必须保留非内容元数据"
if rg -q 'title=\$title' src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt; then
  fail "Android 系统通知日志不应输出通知标题内容"
fi

log "权限与配置失败关闭哨兵"
rg -q "\"vcp-mobile:allow-show-system-notification\"" src-tauri/capabilities/agent-notifications.json \
  || fail "系统通知命令必须使用单独 capability 授权"
if rg -q "\"show_system_notification\"" src-tauri/plugins/vcp-mobile/permissions/all.toml src-tauri/plugins/vcp-mobile/permissions/default.toml; then
  fail "show_system_notification 不应进入 vcp-mobile allow-all/default 聚合权限"
fi
if rg -q "\"run_root_command\"" src-tauri/plugins/vcp-mobile/permissions/all.toml src-tauri/plugins/vcp-mobile/permissions/default.toml; then
  fail "run_root_command 不应进入 vcp-mobile allow-all/default 聚合权限"
fi
if rg -q "\"vcp-mobile:allow-run-root-command\"" src-tauri/capabilities/default.json; then
  fail "run_root_command 必须通过独立 capability 显式授权，不能跟随默认窗口能力"
fi
rg -q "show_system_notification is only supported on Android" src-tauri/plugins/vcp-mobile/src/system.rs \
  || fail "非 Android show_system_notification 必须返回显式错误"
rg -q "attempted: false" src-tauri/plugins/vcp-mobile/src/system.rs \
  || fail "非 Android Android 通知状态必须标记为未尝试"
rg -q "schema_version: u32" src-tauri/src/distributed/tool_registry.rs \
  || fail "禁用工具配置必须读取 schemaVersion"
rg -q "Unsupported disabled tools schemaVersion" src-tauri/src/distributed/tool_registry.rs \
  || fail "禁用工具配置必须拒绝未知 schemaVersion"
rg -q "disable_all_tools" src-tauri/src/distributed/tool_registry.rs \
  || fail "禁用工具配置解析失败必须失败关闭"
rg -q "MAX_CHECK_NEW_TOPICS_DAYS" src-tauri/src/distributed/tools/topic_sponsor.rs \
  || fail "TopicSponsor CheckNewTopics days 必须设置上限"
rg -q "saturating_sub\\(days.saturating_mul\\(MILLIS_PER_DAY\\)\\)" src-tauri/src/distributed/tools/topic_sponsor.rs \
  || fail "TopicSponsor CheckNewTopics cutoff 必须使用饱和计算"

log "Tauri command 注册一致性"
for command in \
  sendToVCP \
  handle_agent_chat_message \
  handle_group_chat_message \
  append_single_message \
  patch_single_message \
  delete_messages \
  truncate_history_after_timestamp \
  get_topics_streamed \
  start_manual_sync \
  stop_sync \
  init_vcp_log_connection \
  send_vcp_log_message \
  get_distributed_status \
  execute_distributed_tool \
  check_for_update \
  check_for_frontend_update
do
  rg -q "$command" src-tauri/src/lib.rs || fail "主 invoke handler 缺少: $command"
done

log "插件与后端互通一致性"
rg -q "tauri_plugin_vcp_mobile::init\\(\\)" src-tauri/src/lib.rs \
  || fail "主后端未注册 tauri-plugin-vcp-mobile"
rg -q "register_android_plugin\\(\"com.vcp.mobile\", \"VcpMobilePlugin\"\\)" src-tauri/plugins/vcp-mobile/src/lib.rs \
  || fail "Rust 插件未注册 Android VcpMobilePlugin"
for bridge in \
  checkAllPermissions \
  requestAndroidPermission \
  moveTaskToBack \
  pickFile \
  showSystemNotification \
  startSensorCollection \
  stopSensorCollection \
  getSensorData \
  acquireWakeLock \
  releaseWakeLock \
  startNetworkMonitoring
do
  rg -q "\"$bridge\"" src-tauri/plugins/vcp-mobile/src/system.rs \
    || fail "Rust 插件 wrapper 缺少 run_mobile_plugin: $bridge"
  rg -q "fun $bridge\\(invoke: Invoke" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePlugin.kt \
    || fail "Android Kotlin 插件缺少命令实现: $bridge"
done
for distributed_bridge in \
  src-tauri/src/distributed/tools/notification.rs \
  src-tauri/src/distributed/tools/agent_message.rs
do
  rg -q "tauri_plugin_vcp_mobile::system::dispatch_system_notification" "$distributed_bridge" \
    || fail "分布式工具未直连 Android 系统通知: $distributed_bridge"
done
rg -q "\"distributed-notification\"" src-tauri/src/distributed/tools/notification.rs \
  || fail "MobileNotification 缺少前端兜底事件"
rg -q "\"vcp-system-event\"" src-tauri/src/distributed/tools/agent_message.rs \
  || fail "AgentMessage 缺少前端系统事件"

log "分布式工具注册一致性"
for tool in \
  DeviceInfoTool \
  NotificationTool \
  ClipboardTool \
  AgentMessageTool \
  MobileAgentMessageTool \
  TopicMemoTool \
  TopicSponsorTool \
  BatteryInfoTool \
  MemoryInfoTool \
  CpuInfoTool \
  GpuInfoTool \
  NetworkInfoTool \
  StorageInfoTool \
  LocationTool \
  MotionSensorTool \
  AmbientSensorTool \
  DeviceStatusSummaryTool
do
  rg -q "$tool" src-tauri/src/distributed/tools/mod.rs || fail "分布式工具未注册: $tool"
done

log "前端通知解析去重检查"
if rg -n "findAgentMessagePayload\\s*=|function findAgentMessagePayload" src/App.vue src/core/composables/useNotificationProcessor.ts; then
  fail "AgentMessage payload 解析 helper 不应回到调用方重复定义"
fi
rg -q "event.payload.androidNotification\\?\\.delivered === true" src/features/distributed/ToolInteractionOverlay.vue \
  && fail "distributed-notification 不能直接信任未校验事件载荷字段"
rg -q "normalizeDistributedNotification" src/features/distributed/ToolInteractionOverlay.vue \
  || fail "distributed-notification 必须先归一化事件载荷"
if rg -q "event.payload.title.length|event.payload.body.length" src/features/distributed/ToolInteractionOverlay.vue; then
  fail "distributed-notification 日志不能直接读取未校验 title/body 字段"
fi
if rg -q "findAgentMessagePayload\\(event.payload\\)" src/features/distributed/ToolInteractionOverlay.vue; then
  fail "distributed-notification 不能再用 AgentMessage 解析器判断当前事件载荷"
fi
rg -q "Agent message event emit failed after notification dispatch" src-tauri/src/distributed/tools/agent_message.rs \
  || fail "AgentMessage 通知副作用后 emit 失败不得返回 Err 触发重试"
if rg -q "Failed to emit agent message event" src-tauri/src/distributed/tools/agent_message.rs; then
  fail "AgentMessage 不应在通知副作用后因事件 emit 失败返回 Err"
fi

log "重复执行类型检查与关键 Rust 测试"
for round in $(seq 1 "$ROUNDS"); do
  log "第 ${round}/${ROUNDS} 轮"
  run pnpm exec vue-tsc --noEmit
  run cargo test --manifest-path src-tauri/Cargo.toml agent_message --lib
  run cargo test --manifest-path src-tauri/Cargo.toml topic_memo --lib
  run cargo test --manifest-path src-tauri/Cargo.toml topic_sponsor --lib
  run cargo test --manifest-path src-tauri/Cargo.toml stream_block_parser --lib
  run cargo test --manifest-path src-tauri/Cargo.toml context_sanitizer --lib
  run cargo test --manifest-path src-tauri/Cargo.toml vcp_log --lib
done

log "并发编译压力检查"
run cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=4

if [[ "$RUN_ANDROID" == "1" ]]; then
  log "Android 原生编译鲁棒性检查"
  (cd src-tauri/gen/android && run bash ./gradlew :tauri-plugin-vcp-mobile:compileDebugKotlin :app:compileUniversalDebugKotlin)
else
  log "跳过 Android 原生编译鲁棒性检查"
  printf '如需覆盖 Android 原生插件，请运行: RUN_ANDROID=1 ROUNDS=%s bash qa/robustness.sh\n' "$ROUNDS"
fi

log "鲁棒性脚本完成"
