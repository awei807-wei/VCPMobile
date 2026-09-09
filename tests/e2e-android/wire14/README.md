# Wire 1.4 Android 真实 E2E Harness

`run-wire14.cjs` 是当前 Wire 1.4 Android 端到端验收入口。它从
`--desktop-root` 加载 VCPChat 的 `VCPMobileSync 1.4.0` 与
`ChatDataServiceFacade`，在临时 `AppData` 中创建全合成 fixture，再由 Android
Debug 应用通过 Tauri IPC/CDP 执行真实同步。脚本只有在所有 hard-gate 场景
真实完成并且清理成功时才返回 0；异常、超时和未支持能力均返回非零。

## 前置条件

- Linux VCPChat 源码目录包含 `VCPMobileSync`、`ChatDataServiceFacade` 和
  当前平台的 `vcp_chat_data_service` 可执行文件。
- 已安装桌面源码依赖；脚本不执行依赖安装，也不修改桌面仓库。
- 一台处于 `device` 状态的 Android 模拟器，Debug 应用（包名
  `com.vcp.avatar.debug`）正在运行且 WebView 开启远程调试；多设备时设置
  `ANDROID_SERIAL`。
- Node.js 22 或更新版本，`adb` 在 `PATH` 中。

## 执行

```bash
node tests/e2e-android/wire14/run-wire14.cjs \
  --desktop-root /home/shiyi/Downloads/VCPChat \
  --evidence /tmp/vcp-wire14-e2e.json
```

可用参数：

- `--timeout-ms`：每次同步的有界等待时间，默认 `180000`。
- `--poll-ms`：状态轮询间隔，默认 `500`。
- `--scale-topics`：规模 fixture 的 topic 数，默认 `1794`；低于 1794 时规模
  场景必然失败，不会伪造等价规模通过。
- `--evidence`：写入脱敏 JSON 证据；父目录会自动创建。

## 当前 hard-gate 场景

1. 两个 owner 共用同一 `topicId` 和 `messageId` 时内容隔离。
2. Linux→Android 与 Android→Linux 新增消息。
3. message/topic tombstone 删除后同步与再次同步均不复活。
4. 头像、经本地 CAS 内容哈希验证后绑定的普通附件、缺失二进制附件，以及非法
   hash 的预期拒绝。
5. 手动 stop 后重启、主动 WebSocket 中断后的有界恢复、final ACK 丢失注入后
   的预期超时拒绝，以及随后重新同步恢复。
6. Android 前后台恢复、强杀后的冷启动恢复。
7. 至少 1794 topic 等价规模，记录耗时、阶段统计和 Android/CDS 峰值内存。
8. 所有变更完成后的最终再次同步为空收敛；Android/CDS 快照不变，且传输观测器
   确认没有额外 pull、push 或 delete。

场景使用合成、带运行 nonce 的 owner/topic/message 标识，避免上一次失败运行
留下的 Android 状态污染本次验收。证据只保留阶段、数量、耗时、状态、错误枚举
和截断哈希；不会写入同步令牌、消息正文、附件内容、路径或原始日志。

## 隔离与清理

- 桌面 `AppData` 和 CDS 日志位于临时目录，脚本收尾递归清理。
- 每次运行生成新的随机同步令牌，只在 Node/插件运行时和 Android 应用私有设置中
  临时使用；收尾恢复原设置，令牌不打印、不写入证据或仓库。
- Android 临时连接档收尾用 `write_settings` 恢复；恢复失败会进入 hard-gate。
- Android 访问地址固定为 `10.0.2.2`，不会注入回环地址；本次创建的 CDP
  `adb forward` 会被移除。
