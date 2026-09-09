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
[[ -f src-tauri/src/vcp_modules/persistence/message_content_storage.rs ]] \
  || fail "缺少消息正文存储兼容层"
for file in \
  src-tauri/src/bootstrap.rs \
  src-tauri/src/vcp_modules/infra/invoke_dispatch.rs \
  src-tauri/src/vcp_modules/infra/invoke_dispatch_tests.rs
do
  [[ -f "$file" ]] || fail "缺少中央 IPC 关键文件: $file"
done
for file in \
  tests/e2e-android/b4-search-perf-gates.test.cjs \
  tests/e2e-android/b4-search-perf-memory.test.cjs \
  tests/e2e-android/b4-search-perf-evidence.test.cjs \
  tests/e2e-android/b4-search-perf-readiness.test.cjs \
  tests/e2e-android/b4-search-perf-rebuild.test.cjs \
  tests/e2e-android/b4-search-perf-restart.test.cjs \
  tests/e2e-android/b4-search-perf-runner.test.cjs \
  tests/e2e-android/b4-search-perf-device.test.cjs \
  tests/e2e-android/b4-search-perf-main.test.cjs \
  tests/e2e-android/b4-search-perf-fixture-cli.test.cjs \
  tests/e2e-android/scripts/adb-env.test.cjs \
  tests/e2e-android/helper-stream-e2e-runner.test.cjs \
  tests/e2e-android/wire14/android-cdp.test.cjs \
  tests/e2e-android/wire14/run-wire14-gates.test.cjs \
  tests/e2e-android/wire14/scale-gate.test.cjs \
  tests/e2e-android/wire14/sync-attempt.test.cjs
do
  [[ -f "$file" ]] || fail "缺少 Android Node 结构测试: $file"
done
rg -q "macro_rules! build_invoke_handler" src-tauri/src/lib.rs \
  || fail "lib.rs 缺少中央 IPC handler 构造入口"
rg -q "vcp_modules::infra::invoke_dispatch::central_invoke_handler\\(handler\\)" src-tauri/src/lib.rs \
  || fail "lib.rs 未连接 central_invoke_handler"
rg -q "pub fn central_invoke_handler<R: Runtime>" src-tauri/src/vcp_modules/infra/invoke_dispatch.rs \
  || fail "central_invoke_handler 必须泛化到 Runtime"
rg -q "reject_before_db_ready = invoke_guard::should_reject_before_db_ready" src-tauri/src/vcp_modules/infra/invoke_dispatch.rs \
  || fail "DB-ready 门禁必须在入口快照并随 Invoke 进入 worker"
rg -q "configure_builder\\(handler\\)" src-tauri/src/bootstrap.rs \
  || fail "bootstrap 未接收中央 IPC handler"
rg -q "\\.invoke_handler\\(handler\\)" src-tauri/src/bootstrap.rs \
  || fail "bootstrap 未把中央 IPC handler 交给 Tauri"
rg -q 'panic = "abort"' src-tauri/Cargo.toml \
  || fail "Release profile 必须保持 panic=abort"
rg -q "central_dispatch_handles_success_argument_error_and_unknown_once" src-tauri/src/vcp_modules/infra/invoke_dispatch_tests.rs \
  || fail "中央 IPC 缺少正常/参数错误/unknown exactly-once 测试"
rg -q "central_dispatch_snapshots_core_not_ready_before_worker_registration" src-tauri/src/vcp_modules/infra/invoke_dispatch_tests.rs \
  || fail "中央 IPC 缺少到达时 DB-ready 快照竞态测试"
rg -q "central_dispatch_runs_sync_handler_after_entry_returns" src-tauri/src/vcp_modules/infra/invoke_dispatch_tests.rs \
  || fail "中央 IPC 缺少同步假 handler 的入口返回顺序测试"
rg -q "central_dispatch_async_barrier_resolves_64_invocations_exactly_once" src-tauri/src/vcp_modules/infra/invoke_dispatch_tests.rs \
  || fail "中央 IPC 缺少 64 并发 exactly-once 测试"
rg -q "normalize_message_content\(&pool\)\.await\?" src-tauri/src/vcp_modules/persistence/database_lifecycle.rs \
  || fail "数据库初始化必须调用消息正文兼容归一化"
rg -q "normalize_legacy_message_content\(pool\)" src-tauri/src/vcp_modules/persistence/database_lifecycle.rs \
  || fail "数据库初始化必须在注册 DbState 前归一化历史 TEXT 消息正文"
rg -q "Component && lifecycleStore\.state === 'READY'" src/App.vue \
  || fail "核心 READY 前不得挂载会触发数据库调用的业务路由"
rg -q "should_reject_before_db_ready" src-tauri/src/vcp_modules/infra/invoke_dispatch.rs \
  || fail "主 invoke handler 必须在 DbState 注册前失败关闭业务命令"
rg -q "database_commands_fail_closed_until_db_state_exists" src-tauri/src/vcp_modules/infra/invoke_guard.rs \
  || fail "DbState 启动门禁缺少数据库命令回归测试"
rg -q "require_db_state\(&app_handle\)\?" src-tauri/src/vcp_modules/infra/settings_manager.rs \
  || fail "settings 读取必须在 DbState 缺失时返回 CORE_NOT_READY 而不是 panic"
rg -q "网络恢复时数据库尚未注册" src-tauri/src/vcp_modules/infra/lifecycle_manager.rs \
  || fail "网络恢复路径必须在数据库注册前延后执行"
rg -q 'decode_message_content\(&row, "content"\)' src-tauri/src/vcp_modules/infra/vcp_client_active_error.rs \
  || fail "启动期活跃生成恢复必须使用兼容解码读取消息正文"
rg -q "finalize_stream_message" src-tauri/src/vcp_modules/infra/vcp_client_active_error.rs \
  || fail "流式错误终结必须委托统一消息终结路径"
rg -q 'ContentCompressor::compress\(&message.content\)' src-tauri/src/vcp_modules/persistence/message_repository_upsert.rs \
  || fail "流式错误回写必须保持 zstd BLOB 存储约定"
if rg -q 'get::<Option<String>, _>\("content"\)' src-tauri/src/vcp_modules/infra; then
  fail "启动恢复不得把 messages.content BLOB 按 Option<String> 读取"
fi
if rg -q "decompress_database_migration" src-tauri/src/vcp_modules/persistence; then
  fail "禁止恢复方向相反的 BLOB→TEXT 启动迁移"
fi
if rg -q "function findAgentMessageToolPayload" src; then
  fail "AgentMessage 工具 payload 查找不应重新拆出平行递归 helper"
fi
rg -q "agentNotificationDedupLock" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobileNotificationCommands.kt \
  || fail "Android AgentMessage 去重锁缺失"
rg -q "AtomicInteger" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobileNotificationCommands.kt \
  || fail "Android AgentMessage 通知 ID 原子计数器缺失"
rg -q "nextAgentMessageNotificationId" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobileNotificationCommands.kt \
  || fail "Android AgentMessage 通知 ID 生成函数缺失"
rg -q "系统通知已发布：id=.*正文长度=" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobileNotificationCommands.kt \
  || fail "Android 系统通知日志必须保留非内容元数据"
rg -q 'AGENT_MESSAGE_CHANNEL_ID = "agent_message_alerts_v2"' src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePluginCore.kt \
  || fail "Android AgentMessage 必须使用新的独立有声通知通道"
rg -q "setSound\\(agentMessageSoundUri, soundAttributes\\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePluginCore.kt \
  || fail "Android AgentMessage 通知通道必须显式设置默认铃声"
rg -q "enableVibration\\(true\\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePluginCore.kt \
  || fail "Android AgentMessage 通知通道必须显式启用振动"
rg -q "hasAgentMessageRingCapability" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePermissionCommands.kt \
  || fail "首次权限门必须检查 AgentMessage 响铃能力"
rg -q "hasSound || hasVibration" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePermissionCommands.kt \
  || fail "响铃能力应接受声音或振动任一可用，避免 OEM 关闭振动后误判未授权"
rg -q "ACTION_APP_NOTIFICATION_SETTINGS" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePermissionCommands.kt \
  || fail "响铃能力异常时必须优先引导到应用通知设置，覆盖定制系统应用级铃声开关"
rg -q "requiredGranted" src/components/layout/PermissionGate.vue \
  || fail "权限门必须保留必需权限聚合判断"
rg -q "status\\.value\\.ring &&" src/components/layout/PermissionGate.vue \
  || fail "通知铃声必须作为首装引导硬性前置，未开启时不得进入下一步"
rg -q "ringBlockingMissing" src/components/layout/PermissionGate.vue \
  || fail "响铃缺失必须展示阻断提示与设置入口"
if rg -q "ringRecommendedMissing|continuing bootstrap with silent" src/components/layout/PermissionGate.vue src/core/stores/appLifecycle.ts; then
  fail "通知铃声不得再作为推荐项放行启动流程"
fi
rg -q "export function hasRequiredStartupPermissions" src/core/stores/appLifecycle.ts \
  || fail "启动编排必须通过显式 helper 聚合硬权限，防止异常路径跳过引导"
rg -q "hasRequiredStartupPermissions\\(permissions, listener\\)" src/core/stores/appLifecycle.ts \
  || fail "启动编排必须把通知监听器纳入硬权限判定"
for permission in notification ring storage battery; do
  rg -q "permissions\\?\\.${permission} === true" src/core/stores/appLifecycle.ts \
    || fail "启动硬权限缺少显式严格判断: $permission"
done
rg -q "listener\\?\\.enabled === true" src/core/stores/appLifecycle.ts \
  || fail "通知监听器必须显式为 true 才能放行启动"
if rg -q "Object\\.values\\(permissions\\)\\.every\\(Boolean\\)" src/core/stores/appLifecycle.ts; then
  fail "启动权限不得使用包含可选诊断字段的 Object.values 全量聚合"
fi
rg -q "vcp-lifecycle" src/components/layout/PermissionGate.vue \
  || fail "权限门必须监听 Android 生命周期恢复，避免设置页返回后状态不刷新"
rg -q "permission-gate-bottom-action" src/components/layout/PermissionGate.vue \
  || fail "权限门底部操作区必须适配虚拟导航栏安全区"
rg -q 'features = \[[^]]*"custom-protocol"' src-tauri/Cargo.toml \
  || fail "Android 可安装 debug APK 必须启用 Tauri custom-protocol，避免离线启动时请求 http://localhost:1420"
if rg -q 'title=\$title' src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobileNotificationCommands.kt; then
  fail "Android 系统通知日志不应输出通知标题内容"
fi

log "权限与配置失败关闭哨兵"
run python3 qa/check_vcp_mobile_acl.py
rg -q "\"vcp-mobile:allow-show-system-notification\"" src-tauri/capabilities/agent-notifications.json \
  || fail "系统通知命令必须使用单独 capability 授权"
if rg -q "show[_-]system[_-]notification" src-tauri/plugins/vcp-mobile/permissions/all.toml src-tauri/plugins/vcp-mobile/permissions/default.toml; then
  fail "show_system_notification 不应进入 vcp-mobile allow-all/default 聚合权限"
fi
if rg -q "run[_-]root[_-]command" src-tauri/plugins/vcp-mobile/permissions/all.toml src-tauri/plugins/vcp-mobile/permissions/default.toml; then
  fail "run_root_command 不应进入 vcp-mobile allow-all/default 聚合权限"
fi
if rg -q "\"vcp-mobile:allow-run-root-command\"" src-tauri/capabilities/default.json; then
  fail "run_root_command 必须通过独立 capability 显式授权，不能跟随默认窗口能力"
fi
rg -q "show_system_notification is only supported on Android" src-tauri/plugins/vcp-mobile/src/system.rs \
  || fail "非 Android show_system_notification 必须返回显式错误"
rg -q "attempted: false" src-tauri/plugins/vcp-mobile/src/system.rs \
  || fail "非 Android Android 通知状态必须标记为未尝试"
rg -q "ENABLED_CONFIG_SCHEMA_VERSION: u32" src-tauri/src/distributed/tool_registry_config.rs \
  || fail "禁用工具配置必须读取 schemaVersion"
rg -q "不支持的工具配置 schemaVersion" src-tauri/src/distributed/tool_registry_config.rs \
  || fail "禁用工具配置必须拒绝未知 schemaVersion"
rg -q "保持全部拒绝" src-tauri/src/distributed/tool_registry.rs \
  || fail "禁用工具配置解析失败必须失败关闭"
rg -q "MAX_CHECK_NEW_TOPICS_DAYS" src-tauri/src/distributed/tools/topic_sponsor.rs \
  || fail "MobileTopicSponsor CheckNewTopics days 必须设置上限"
rg -q "saturating_sub\\(days.saturating_mul\\(MILLIS_PER_DAY\\)\\)" src-tauri/src/distributed/tools/topic_sponsor_handlers.rs \
  || fail "MobileTopicSponsor CheckNewTopics cutoff 必须使用饱和计算"

log "分布式长连接保活哨兵"
rg -q "DISTRIBUTED_HEARTBEAT_INTERVAL" src-tauri/src/distributed/client.rs \
  || fail "分布式 WebSocket 必须保留主动心跳间隔"
rg -q "DISTRIBUTED_HEARTBEAT_TIMEOUT" src-tauri/src/distributed/client.rs \
  || fail "分布式 WebSocket 必须保留心跳超时阈值"
rg -q "Message::Ping\\(Vec::new\\(\\)\\.into\\(\\)\\)" src-tauri/src/distributed/client_session.rs \
  || fail "分布式 WebSocket 必须主动发送标准 Ping"
rg -q "没有收到 WebSocket 帧" src-tauri/src/distributed/client_session.rs \
  || fail "分布式 WebSocket 半开连接必须主动超时"
rg -q "is_distributed_connection_stale" src-tauri/src/distributed/client.rs \
  || fail "分布式心跳超时策略必须有可测 helper"
rg -q "recover_distributed_node_after_network_restore" src-tauri/src/vcp_modules/infra/lifecycle_manager.rs \
  || fail "网络恢复必须能触发完整分布式生命周期恢复"
rg -q "recover_distributed_node_after_network_restore\\(&handle\\)" src-tauri/src/bootstrap.rs \
  || fail "网络恢复事件不能只发送空 session reconnect 信号"
rg -q "private fun start\\(context: Context\\): Boolean" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/ForegroundServiceLifecycleCoordinator.kt \
  || fail "Android 前台服务启动必须有独立可验证的失败处理入口"
rg -q "ForegroundServiceLifecycleCoordinator" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/ForegroundGuardian.kt \
  || fail "ForegroundGuardian 必须构造前台服务生命周期协调器"
rg -q "startPending = false" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/ForegroundServiceLifecycleCoordinator.kt \
  || fail "Android 前台服务启动失败必须清理待处理状态"
rg -q "Log\\.e\\(TAG, \"startFgs 失败\", error\\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/ForegroundServiceLifecycleCoordinator.kt \
  || fail "Android 前台服务后台启动限制必须显式处理"
if rg -q "degrading to normal service" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/ForegroundGuardian.kt; then
  fail "前台服务启动失败不能静默降级为普通后台服务"
fi
rg -q "onTaskRemoved" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/StreamKeepaliveService.kt \
  || fail "任务被移除时必须有分布式保活恢复入口"
rg -q "createRecoveryIntent" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/StreamKeepaliveService.kt \
  || fail "前台保活服务必须提供恢复 Intent"
rg -q "distributed_keepalive_active" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/StreamKeepaliveService.kt \
  || fail "分布式保活意图必须持久化供开机/包更新恢复使用"
rg -q "promoteToForeground\(buildBootstrapNotification\(\)\)" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/StreamKeepaliveService.kt \
  || fail "前台服务必须在 onCreate 最早阶段用最小通知完成提升"
rg -q "startPending" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/ForegroundServiceLifecycleCoordinator.kt \
  || fail "前台服务重复启动必须合并为单个待处理请求"
rg -q "onAppForegroundChanged" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/ForegroundGuardian.kt \
  || fail "前台服务必须根据应用可见状态延迟到后台转换时启动"
rg -q "appInForeground" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/ForegroundGuardian.kt \
  || fail "前台守护者缺少应用可见状态门控"
rg -q "ACTION_REFRESH_NOTIFICATION" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/ForegroundServiceLifecycleCoordinator.kt \
  || fail "已运行前台服务的通知更新必须走普通 Service 更新路径"
rg -q 'android:stopWithTask="false"' src-tauri/plugins/vcp-mobile/android/src/main/AndroidManifest.xml \
  || fail "任务移除时前台服务必须保留自身生命周期并避免递归重启"
[[ -f src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/receiver/BootReceiver.kt ]] \
  || fail "缺少 BootReceiver 分布式保活恢复入口"
rg -q "RECEIVE_BOOT_COMPLETED" src-tauri/plugins/vcp-mobile/android/src/main/AndroidManifest.xml \
  || fail "缺少开机恢复权限声明"
rg -q "ACTION_BOOT_COMPLETED" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/receiver/BootReceiver.kt \
  || fail "BootReceiver 必须处理开机完成事件"
rg -q 'android:name="\.VcpApplication"' src-tauri/gen/android/app/src/main/AndroidManifest.xml \
  || fail "崩溃诊断必须在 Application 阶段安装"
rg -q "CrashDiagnostics.install\(this\)" src-tauri/gen/android/app/src/main/java/com/vcp/avatar/VcpApplication.kt \
  || fail "VcpApplication 未安装早期崩溃诊断"
rg -q "START_NOT_STICKY" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/SseProxyService.kt \
  || fail "SSE helper 不得在无会话时被系统粘性重启"
if rg -U -q 'registerActivityLifecycleCallbacks\(activityLifecycleCallbacks\)\s*startHelperServiceInternal\(\)' src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobilePluginCore.kt; then
  fail "SSE helper 不得在插件初始化阶段无条件拉起前台服务"
fi
if rg -q 'setSmallIcon\(applicationInfo\.icon\)' \
  src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/VcpMobileNotificationCommands.kt \
  src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/StreamKeepaliveService.kt \
  src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile/service/SseProxyService.kt; then
  fail "Android 前台通知不得使用 launcher 自适应图标作为 small icon"
fi
python3 scripts/verify_android_apk.py --help >/dev/null \
  || fail "真机 APK ABI/16KB 页对齐校验脚本不可执行"

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
rg -q "tauri_plugin_vcp_mobile::init\\(\\)" src-tauri/src/bootstrap.rs \
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
  rg -q "fun $bridge\\(invoke: Invoke" src-tauri/plugins/vcp-mobile/android/src/main/java/com/vcp/mobile \
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
rg -q "select_apk_asset" src-tauri/src/vcp_modules/updater/update_manager.rs \
  || fail "APK 更新必须通过独立 asset 选择函数"
rg -q "rejects_debug_and_non_apk_assets" src-tauri/src/vcp_modules/updater/update_manager.rs \
  || fail "APK asset 选择必须覆盖 debug 排除测试"
rg -q "test_daily_note_static_and_stream_parsers_agree" src-tauri/src/vcp_modules/chat/stream_block_parser.rs \
  || fail "DailyNote 静态与流式解析一致性测试缺失"

log "前端单元测试"
run pnpm test:unit -- --maxWorkers=1 --no-file-parallelism --maxConcurrency=1

log "B4/CDP Node 结构测试"
run node --test \
  tests/e2e-android/b4-search-perf-gates.test.cjs \
  tests/e2e-android/b4-search-perf-memory.test.cjs \
  tests/e2e-android/b4-search-perf-evidence.test.cjs \
  tests/e2e-android/b4-search-perf-readiness.test.cjs \
  tests/e2e-android/b4-search-perf-rebuild.test.cjs \
  tests/e2e-android/b4-search-perf-restart.test.cjs \
  tests/e2e-android/b4-search-perf-runner.test.cjs \
  tests/e2e-android/b4-search-perf-device.test.cjs \
  tests/e2e-android/b4-search-perf-main.test.cjs \
  tests/e2e-android/b4-search-perf-fixture-cli.test.cjs \
  tests/e2e-android/scripts/adb-env.test.cjs \
  tests/e2e-android/helper-stream-e2e-runner.test.cjs \
  tests/e2e-android/wire14/android-cdp.test.cjs \
  tests/e2e-android/wire14/run-wire14-gates.test.cjs \
  tests/e2e-android/wire14/scale-gate.test.cjs \
  tests/e2e-android/wire14/sync-attempt.test.cjs

log "B4 fixture Python 测试"
run python3 tests/e2e-android/scripts/b4_fixture_test.py

log "前端类型检查"
run pnpm exec vue-tsc --noEmit

log "重复执行关键 Rust 测试"
for round in $(seq 1 "$ROUNDS"); do
  log "第 ${round}/${ROUNDS} 轮"
  run cargo test --manifest-path src-tauri/Cargo.toml agent_message --lib
  run cargo test --manifest-path src-tauri/Cargo.toml topic_memo --lib
  run cargo test --manifest-path src-tauri/Cargo.toml topic_sponsor --lib
  run cargo test --manifest-path src-tauri/Cargo.toml stream_block_parser --lib
  run cargo test --manifest-path src-tauri/Cargo.toml daily_note --lib
  run cargo test --manifest-path src-tauri/Cargo.toml update_manager --lib
  run cargo test --manifest-path src-tauri/Cargo.toml context_sanitizer --lib
  run cargo test --manifest-path src-tauri/Cargo.toml vcp_log --lib
done

log "并发编译压力检查"
run cargo test --manifest-path src-tauri/Cargo.toml --lib -- --test-threads=4

if [[ "$RUN_ANDROID" == "1" ]]; then
  log "Android 原生单测与编译鲁棒性检查"
  (cd src-tauri/gen/android && run bash ./gradlew :tauri-plugin-vcp-mobile:testDebugUnitTest :app:compileUniversalDebugKotlin)
else
  log "跳过 Android 原生编译鲁棒性检查"
  printf '如需覆盖 Android 原生插件，请运行: RUN_ANDROID=1 ROUNDS=%s bash qa/robustness.sh\n' "$ROUNDS"
fi

log "鲁棒性脚本完成"
