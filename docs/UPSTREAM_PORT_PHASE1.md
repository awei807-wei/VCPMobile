# VCPMobile 首批低风险上游搬运施工图
## 小模型执行版 · 固定源文件 · 禁止扩大范围

- 编制日期：2026-09-09。
- 目标仓库：`awei807-wei/VCPMobile`。
- 实际 fork 起点：`bae00da0e3669de9592c838f1b7ff4592591c78d`（依用户指令，从执行时的本地 `main` 创建）。
- 原施工图接口核对基线：`793ec44f405a2f4f1529629dbc8096cf56750812`；它仍用于下方来源证据，不作为本次改动范围的 Git diff 起点。
- 上游仓库：`MRiecy/VCPMobile`。
- 唯一上游取件版本：`4d53b548572ac4d34c4c69f83188605a24bf99e2`。
- 建议工作分支：`port/upstream-low-risk-20260909`。
- 将本文件放到目标仓库 `docs/UPSTREAM_PORT_PHASE1.md`，它是本任务唯一施工与验收台账。不要另建 PLAN、REPORT、SUMMARY 等派生文档。

> 交付物不是“全量同步上游”，而是：五个独立组件/工具原样移植、四处局部界面修复、一个带连接隔离的只读日志中心。Wire、数据库、渲染主链、OTA 等本轮均不改。
>
> 本文同时是施工图与执行验收台账。当前 T00–T07 已完成并验证，T08 已生成并校验 APK，但真机、测试服务器等人工验收仍为 `NOT_RUN`；详细状态以第 10 节为准。

## 0. 给执行模型的入口指令

直接把下面这段和本文件路径交给执行模型：

```text
读取 docs/UPSTREAM_PORT_PHASE1.md。
只执行本文件列出的首批搬运，不做全量上游合并。
先检查固定基线和文件白名单，再按 T00 → T08 顺序执行。
一次只推进一个任务卡；需要代码时照抄对应附录，不另行设计实现。
源文件必须取指定完整 commit，不取最新 main，不按 commit 标题猜行为。
只允许“指定文件原样新增”和“指定片段修改”，禁止整文件覆盖已有核心代码。
完成一张卡就运行该卡的检查，并在同一文件的台账里记录实际结果。
缺失锚点、基线变化、依赖缺失、类型不匹配、测试失败，都记录 BLOCKED；禁止扩大改动绕过。
不打印密码/API Key，不访问真实用户数据库，不调用服务器清空接口。
不 push、不合并 main、不发 Release、不安装到用户正在使用的应用上。
遇到已存在但与预期不同的文件，停止；不能用上游版本强行盖掉。
只有实际运行通过的检查才能写 PASS；没运行写 NOT_RUN，环境缺失写 BLOCKED_ENV。
最终报告只写：完成任务、修改文件、验证命令与结果、未完成项；不要宣称已与上游全面对齐。
```

### 0.1 四种行为，不得混淆

| 标记 | 允许做什么 | 禁止做什么 |
|---|---|---|
| COPY_EXACT | 从固定上游取单个文件，字节保持一致 | 顺带 cherry-pick 整条提交、格式化源文件、修改实现 |
| PATCH_LOCAL | 在指定 fork 文件替换指定片段 | 用上游同名大文件覆盖、修改其他相似片段 |
| ADD_ADAPTED | 新增文件，采用本文完整适配代码 | 自己重新设计状态机、简化隔离检查 |
| KEEP | 完全保留 | 为通过测试顺手修改 |

“原样搬运”不是“免测试”。只新增了工具文件，也不能报告相应整套业务功能已实现。

### 0.2 本轮业务决定已经固定

1. 日志中心只读：查看、增量拉取、关键词筛选、正反序、暂停/继续、全量刷新、复制可见日志、行数上限。
2. 不提供“清空服务器日志”；为避免异步清屏语义额外复杂化，本轮也不提供“清空本地显示”。
3. 使用当前 Settings 的管理员凭据，不新增账号系统、不修改连接配置结构。
4. 切线路、改服务器地址、改管理员凭据都视为新日志会话；旧结果必须丢弃。
5. 日志页退出、被另一业务页盖住、应用进入后台、用户暂停时，不再启动新轮询；已经发出的请求可能继续到返回/超时，结果不得回写。
6. 同一有效会话最多一个请求；包含过期请求在内，最多两个尚未完成的 invoke。失效不等于取消，不能伪造“已取消网络请求”。
7. 每次重新进入可拉取状态，第一轮全量；后续才增量。这里有意不继承上游的跨开关页面增量续拉策略。
8. `nameHue.ts`、`mention.ts` 只达到工具库就绪，不接整套头像替换或群聊邀请发言。

## 1. 冻结边界与来源

### 1.1 禁止修改的范围

本轮不修改以下文件/目录：

```text
package.json
pnpm-lock.yaml
pnpm-workspace.yaml
src-tauri/Cargo.toml
src-tauri/Cargo.lock
.github/
src-tauri/gen/android/
src-tauri/plugins/
src-tauri/capabilities/
src-tauri/migrations/
src-tauri/src/distributed/
src-tauri/src/vcp_modules/persistence/
src-tauri/src/vcp_modules/sync/
src-tauri/src/vcp_modules/updater/
src-tauri/src/vcp_modules/chat/
src-tauri/src/vcp_modules/infra/settings_manager.rs
src-tauri/src/vcp_modules/infra/invoke_dispatch.rs
src-tauri/src/vcp_modules/infra/invoke_guard.rs
src/core/stores/settings.ts
src/core/stores/connectionProfiles.ts
src/core/stores/connectionSwitchGuard.ts
src/core/stores/appLifecycle*.ts
src/features/dailynote/
src/features/chat/MessageRenderer.vue
src/components/ui/BottomSheet.vue
```

禁止改锁文件、增加依赖、放宽 ACL、加入通配权限、删失败测试、加 `@ts-ignore` / `@ts-nocheck` / `as any` 掩盖错误。禁止全仓格式化。禁止执行 `git reset --hard`、`git clean -fd`、强推或清理用户数据。

### 1.2 本次源码检查发现的实际接缝

| 位置 | 当前事实 | 本文的固定处理 |
|---|---|---|
| 样式入口 | fork 在 `src/main.ts` 导入样式，不是上游的 `src/appStyles.ts` | 只交换 main.ts 中两条导入 |
| 群成员设置 | fork 已拆出 `GroupMembersSection.vue` | 不改上游原来使用的 GroupSettingsView 大文件 |
| BottomSheet | fork 没有 `compact` 属性，实例使用固定 modalId | 日志页不传 compact；只留一个 BottomSheet，行数选择改原生 select |
| 日志前端 | 上游 Store 保存 offset、尾部半行，未对本 fork 的连接切换做隔离 | 使用附录 C 的完整 fork 适配 Store |
| 日志后端 | 读取 Settings 所需字段已存在；read_settings 支持泛型 Runtime | 新模块只读现有配置，增加期望连接校验，不改 Settings |
| invoke 入口 | fork 已有中央 handler 和启动门禁 | 只注册一个读命令，保留原调度与门禁 |

来源证据在第 11 节，全部钉到这两个 commit。不要再按上游目录猜 fork 的落点。

## 2. 文件白名单

除本表外不能改任何受版本控制的文件。构建目录、node_modules 等忽略目录不属于源码交付，但不允许借此保存用户凭据或日志。

### 2.1 可新增文件

```text
src/components/ui/RefreshButton.vue
src/core/utils/nameHue.ts
src/core/utils/mention.ts
src/features/logcenter/logText.ts
src-tauri/src/vcp_modules/infra/http_clients.rs
src-tauri/src/vcp_modules/infra/admin_api.rs
src-tauri/src/vcp_modules/logcenter/mod.rs
src-tauri/src/vcp_modules/logcenter/log_service.rs
src/features/logcenter/requestEpoch.ts
src/features/logcenter/logCenterStore.ts
src/features/logcenter/LogCenterView.vue
src/tests/unit/port/DirectAssets.test.ts
src/tests/unit/port/RefreshButton.test.ts
src/tests/unit/port/LowRiskUi.test.ts
src/tests/unit/port/RequestEpoch.test.ts
src/tests/unit/port/LogCenterStore.test.ts
src/tests/unit/port/LogCenterIntegration.test.ts
docs/UPSTREAM_PORT_PHASE1.md
```

### 2.2 可局部修改的已有文件

```text
src/main.ts
src/features/rag/RagObserver.vue
src/features/chat/StagedAttachmentPreview.vue
src/features/agent/GroupMembersSection.vue
src-tauri/src/vcp_modules/infra/mod.rs
src-tauri/src/vcp_modules/mod.rs
src-tauri/src/lib.rs
src/core/stores/overlay.ts
src/components/FeatureOverlays.vue
src/components/layout/RightSidebar.vue
```

### 2.3 原样复制文件的 Git blob 校验值

取件 SHA 和文件 blob SHA 不是一回事：前者固定版本，后者验证取来的文件字节。

| 文件 | 预期 Git blob SHA |
|---|---|
| `src/components/ui/RefreshButton.vue` | `17a2ff2753f5681c774105ae6e9ab5c989ec4254` |
| `src/core/utils/nameHue.ts` | `dbb972edfb2551bacb3af104c43c021b58e95a84` |
| `src/core/utils/mention.ts` | `d6fcf0e44ed68a4c658eb713dac928b8409b41d2` |
| `src/features/logcenter/logText.ts` | `8d05a478b09cd5cfe43076a3696b9279865a0941` |
| `src-tauri/src/vcp_modules/infra/http_clients.rs` | `352651f2cf9b82976a385c81446dda82a3192638` |
| `src-tauri/src/vcp_modules/infra/admin_api.rs` | `5a36015ec40153752fbdf7988580319ac8e55c67` |

前五个文件属于 T01；`admin_api.rs` 属于 T03。六个文件交付时都必须与上表字节一致。其他新增文件允许按本文适配，不能再声称是原样移植。

## 3. T00 — 冻结基线与建立基线检查

**前置：** 运行位置是 fork 仓库根目录。先读适用目录下已有的项目维护说明；若与本文范围冲突，停止报告，不擅自选一套覆盖另一套。

首次执行下面的 Bash。只运行一次；恢复任务时不重复建分支。

```bash
set -euo pipefail
BASE=bae00da0e3669de9592c838f1b7ff4592591c78d
UP=4d53b548572ac4d34c4c69f83188605a24bf99e2
BRANCH=port/upstream-low-risk-20260909
DOC=docs/UPSTREAM_PORT_PHASE1.md

[ "$(git rev-parse --show-toplevel)" = "$PWD" ] || { echo 'STOP: 不在仓库根目录'; exit 1; }
[ "$(git rev-parse HEAD)" = "$BASE" ] || { echo 'STOP: fork HEAD 已变化，不能套用旧基线'; exit 1; }
# 允许本施工图本身未提交，其他文件必须干净。
[ -z "$(git status --porcelain -- . ":(exclude)$DOC")" ] || { echo 'STOP: 有其他未提交改动'; exit 1; }
case "$(git remote get-url origin)" in
  https://github.com/awei807-wei/VCPMobile|https://github.com/awei807-wei/VCPMobile.git|git@github.com:awei807-wei/VCPMobile.git) ;;
  *) echo 'STOP: origin 不是指定 fork'; exit 1 ;;
esac

git fetch --no-tags https://github.com/MRiecy/VCPMobile.git "$UP"
git cat-file -e "$UP^{commit}"
git show-ref --verify --quiet "refs/heads/$BRANCH" && { echo 'STOP: 工作分支已存在，按恢复流程处理'; exit 1; }
git switch -c "$BRANCH"
```

Git 的取件方式使用固定树中的指定文件，不进行分支合并；Git 文档参考第 11 节 E12。

**恢复流程：** 当前分支必须已经是上述工作分支；确认 `BASE` 是当前 HEAD 祖先，以 `FINAL = False` 运行附录 E 的阶段检查，然后按第 10 节找到第一个未完成任务。不要切回 main、不要重置 HEAD。出现来源不明的分支改动时停止。

### 3.1 基线验证，改代码前运行

```bash
node --version
pnpm --version
cargo --version
rustc --version
java -version
pnpm install --frozen-lockfile
pnpm exec vue-tsc --noEmit
pnpm test:run
cargo check --manifest-path src-tauri/Cargo.toml --locked
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib
```

这些命令逐条运行、逐条记结果，不允许只看最后一个退出码。缺 pnpm、Rust、Java、系统构建依赖或者网络下载失败，记录 `BLOCKED_ENV`；不要为此更新锁文件或改 CI。基线已有失败也先记录，不把它当成新任务顺手修复。

**完成条件：** 基线固定、来源可读取、工作区无其他用户改动，前端和 Rust 基线检查通过。环境阻塞可做只读准备，但不能标记实现验证完成。

## 4. T01 — 原样加入五个独立文件

### 4.1 使用取件助手，不靠模型重抄源代码

下面 Python 片段只用于从 Git 对象读取固定文件。执行 T01 时 `TASK = "T01"`；执行 T03 时把这一行改为 `"T03"`，其他内容不改。它不会覆盖不同内容的已有文件。无需把助手另存为仓库脚本。

```python
from pathlib import Path
import subprocess

TASK = "T01"
UP = "4d53b548572ac4d34c4c69f83188605a24bf99e2"
FILES = {
    "src/components/ui/RefreshButton.vue": "17a2ff2753f5681c774105ae6e9ab5c989ec4254",
    "src/core/utils/nameHue.ts": "dbb972edfb2551bacb3af104c43c021b58e95a84",
    "src/core/utils/mention.ts": "d6fcf0e44ed68a4c658eb713dac928b8409b41d2",
    "src/features/logcenter/logText.ts": "8d05a478b09cd5cfe43076a3696b9279865a0941",
    "src-tauri/src/vcp_modules/infra/http_clients.rs": "352651f2cf9b82976a385c81446dda82a3192638",
    "src-tauri/src/vcp_modules/infra/admin_api.rs": "5a36015ec40153752fbdf7988580319ac8e55c67",
}
selected = list(FILES)[:5] if TASK == "T01" else [list(FILES)[5]] if TASK == "T03" else None
if selected is None:
    raise SystemExit("STOP: unknown task")
root = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
prepared = []
for relative in selected:
    path = root / relative
    for item in [path, *path.parents]:
        if item == root:
            break
        if item.is_symlink():
            raise SystemExit(f"STOP: symlink in target: {relative}")
    content = subprocess.check_output(["git", "show", f"{UP}:{relative}"], cwd=root)
    digest = subprocess.check_output(["git", "hash-object", "--stdin"], input=content, cwd=root).decode().strip()
    if digest != FILES[relative]:
        raise SystemExit(f"STOP: source hash mismatch: {relative}")
    if path.exists() and (not path.is_file() or path.read_bytes() != content):
        raise SystemExit(f"STOP: different existing file: {relative}")
    prepared.append((path, content))
# 所有源文件和目标都通过检查后才写入。
for path, content in prepared:
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    print("READY", path.relative_to(root))
```

在 `src-tauri/src/vcp_modules/infra/mod.rs` 中，紧邻其他 `pub mod` 新增一次：

```rust
pub mod http_clients;
```

不要替换已有聊天、同步、OTA 的 HTTP 客户端，不改变它们的超时或重定向策略。

### 4.2 本卡测试

在白名单的 `DirectAssets.test.ts`、`RefreshButton.test.ts` 中建立以下用例。使用项目现有 Vitest 和 Vue Test Utils，不安装新包。

| 对象 | 固定输入/动作 | 必须断言 |
|---|---|---|
| nameInitial | `"  张三"`、`"alice"`、`"  "`、`"𠮷野"` | `张`、`A`、`?`、`𠮷` |
| nameHue | 对同一个名字调用两次 | 结果相同且处于 0..359 |
| mention | `"@Alice ＠Bob @Alice"`，成员 Alice/Bob | 输出两个成员 id，按首次出现顺序，不重复 |
| mention | 名字 `Ab`、`Abc`，文本 `@Abc` | 最长名字优先 |
| mention | splitMentionSegments 后重拼文本 | 与输入逐字符相同 |
| 日志分块 | carry=`半`，content=`行\n尾` | lines=`["半行","尾"]`，trailing=`尾` |
| 日志清理 | ANSI 包裹的 `[ERROR] x` | ANSI 被去掉，levelOf 返回 error |
| 日志关键词 | `aBc` 搜索 `b` | 命中段是原大小写的 `B`，拼回仍为 aBc |
| 刷新按钮 | 无点击，loading 从 false→true | 不旋转，不发 refresh |
| 刷新按钮 | 点击→loading true→一次 animationiteration | 发一次 refresh，仍旋转 |
| 刷新按钮 | loading false→下一次 animationiteration | 移除旋转 class |
| 刷新按钮 | disabled=true 后点击 | 不发 refresh |

点击与旋转要挂载真实组件并触发事件，不允许只 `toContain` 源代码。HTTP 注册表原文件带有单元测试，运行对应 Rust 测试。

```bash
pnpm exec vitest run src/tests/unit/port/DirectAssets.test.ts src/tests/unit/port/RefreshButton.test.ts
pnpm exec vue-tsc --noEmit
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib http_clients
```

**本卡只证明组件/工具已加入和测试通过，不证明整套日志中心或群聊提及已经完成。**

## 5. T02 — 四处局部修复

本卡拆成四个小动作。每次修改前，旧片段必须在指定文件里恰好命中一次；如果已是新片段则记录已完成；旧新同时存在、命中多次、都不命中都停止。不要扩大搜索替换范围。

### T02-a：样式导入顺序

文件：`src/main.ts`。参考上游 `22f4d3d`，但不搬上游 `appStyles.ts`。

```ts
// 旧
import 'virtual:uno.css'
import "@unocss/reset/tailwind.css"

// 新
import "@unocss/reset/tailwind.css"
import 'virtual:uno.css'
```

只交换这两行。其他 imports、runtime diagnostics、OTA 启动确认完全不动。

### T02-b：附件删除徽标不被裁剪

文件：`src/features/chat/StagedAttachmentPreview.vue`。参考 `b17ebf7`。

```html
<!-- 旧 -->
class="relative shrink-0 rounded-xl overflow-hidden"
<!-- 新 -->
class="relative shrink-0 rounded-xl"
```

只改最外层这一个 class。不要移除图片内部或 loading overlay 的圆角/裁剪，也不要改附件上传与发送门禁。

### T02-c：RAG 卡片收起后立即定位

文件：`src/features/rag/RagObserver.vue`。参考 `23b41ff`。

```ts
// 分别替换这两条；不要动 Tab 切换时的平滑滚动。
subCardEl.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
cardEl.scrollIntoView({ behavior: 'smooth', block: 'nearest' });

// 新
subCardEl.scrollIntoView({ behavior: 'instant', block: 'nearest' });
cardEl.scrollIntoView({ behavior: 'instant', block: 'nearest' });
```

### T02-d：群成员区域限高

文件：`src/features/agent/GroupMembersSection.vue`。参考 `3a8f528`。上游原补丁的位置不同，不能改错文件。

```html
<!-- 旧 -->
<div class="card-modern overflow-hidden !p-0">
<!-- 新 -->
<div class="card-modern !p-0 max-h-80 overflow-y-auto vcp-scrollable">
```

保留当前组件的 props、成员标签、头像 owner 参数和所有事件。

### T02 验证

`LowRiskUi.test.ts` 至少覆盖：附件删除事件仍携带原 index；群成员 toggle 仍返回原 agentId；RAG 卡片收起的两个定位调用使用 instant；原有 Tab 切换行为不改。源码断言只能作为补充，附件/成员事件必须做组件行为测试。

```bash
pnpm exec vitest run src/tests/unit/port/LowRiskUi.test.ts
pnpm exec vue-tsc --noEmit
pnpm build
git diff --check
```

视觉验收留到 T08：检查删除徽标、按钮底色、长群成员列表和 RAG 定位。DOM 测试不等于真实 WebView 视觉通过。

## 6. T03 — 新增只读日志后端

**前置：T01/T02 通过。** 不搬整个业务模块 commit。

### 6.1 固定动作

1. 用 T01 取件助手执行 `TASK = "T03"`，原样新增 `infra/admin_api.rs`。
2. 新增 `src-tauri/src/vcp_modules/logcenter/log_service.rs`，完整采用附录 A。
3. 新增 `src-tauri/src/vcp_modules/logcenter/mod.rs`，内容固定为：

```rust
mod log_service;
pub use log_service::logcenter_fetch;
```

4. `src-tauri/src/vcp_modules/infra/mod.rs` 增加 `pub mod admin_api;`，仅此一条。
5. `src-tauri/src/vcp_modules/mod.rs` 的子领域声明区增加 `pub mod logcenter;`，保留 fork 的所有现有声明及 owner_lock。
6. `src-tauri/src/lib.rs` 增加 import：

```rust
use vcp_modules::logcenter::logcenter_fetch;
```

在唯一的 `tauri::generate_handler![` 命令清单里加入一次 `logcenter_fetch,`。不要重建 `build_invoke_handler`，不要删除原有命令，不要改 invoke_dispatch / invoke_guard。

### 6.2 IPC 参数约定，必须前后端一致

| 前端字段 | Rust 参数 | 规则 |
|---|---|---|
| incremental | incremental: bool | 第一次 false，后续正常增量 true |
| offset | offset: u64 | 全量 0；增量用服务器返回的 offset，不按字符串长度计算 |
| expectedProfileId | expected_profile_id: String | 当前激活配置 id |
| expectedServerUrl | expected_server_url: String | 仅用于和后台配置比对，不能作为网络请求的直接目标 |

实际请求目标、管理员用户名、管理员密码都从 Rust `read_settings` 返回的同一个配置快照产生。前端不传密码。配置切换中或者快照不匹配，返回明确错误，不降级、不转发到另一个服务器。

### 6.3 为什么本卡不用原样复制上游 log_service

这里只做三项有意适配：删除写操作；校验预期连接；按 chunk 累加限制响应大小。不能把 `resp.bytes().await` 之后再检查大小写成“读取过程有内存上限”。附录 A 在追加 chunk 前检查累计量，整个请求有 30 秒总超时。

注意：连接校验只保证这次请求使用一致的配置快照，不会让已经发出的 HTTP 请求瞬间取消；旧结果隔离由 T04 共同负责。

### 6.4 验证

```bash
cargo check --manifest-path src-tauri/Cargo.toml --locked
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib logcenter
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib admin_api
```

检查新增业务源码和 lib 注册表中不存在 `logcenter_clear_server`。原有同步日志清理命令属于既有功能，不能误删。

**停止条件：** 泛型参数或 Settings API 不匹配、命令注册编译失败、需要修改 Cargo 才能通过。按固定基线理论上已有依赖；出现这类情况先记录实际错误，不猜依赖版本。

## 7. T04 — 日志 Store 与请求隔离

新增以下两个文件，分别完整照抄附录 B、C：

```text
src/features/logcenter/requestEpoch.ts
src/features/logcenter/logCenterStore.ts
```

不要先搬上游 Store 再凭感觉修几行；本 fork 的连接切换就是这里需要保留的特性。本文适配版不导入 connectionProfiles Store，只读取更小的 connectionSwitchGuard，避免把聊天、同步等依赖引入日志 Store。

### 7.1 固定状态语义

| 事件 | 必须发生什么 |
|---|---|
| 首次打开且是顶层业务页 | 全量读，成功后每 3 秒增量轮询 |
| 连续点击刷新 | 请求结果按代际隔离，实际未完成 invoke 总数不超过 2 |
| 线路开始切换 | 立即清空日志、路径、offset、半行；旧结果失效 |
| 线路切换结束 | 从当前配置重新全量读取，不复用旧 offset |
| 同一个配置 id 修改 URL 或管理员凭据 | 同样清空并新建有效代际 |
| 隐藏/退出/后台/暂停 | 不再启动新请求；旧响应不写入 Store |
| 恢复可见/前台/继续 | 第一轮全量，然后恢复增量 |
| 旧请求晚成功或晚失败 | 只能释放自己的在途名额，不改新会话的内容、错误和 loading |
| 服务端返回 needFullReload | 增量轮转只触发一次立即全量；全量仍要求重载则按错误退避，不能死循环 |
| 失败 | 下一次间隔 6、12、24、30 秒封顶；成功回到 3 秒 |
| 缓冲 | 受行数、单行字符数、总字符数三层限制；发生长行截断时有省略标记 |
| 日志和凭据 | 只在内存中；localStorage 只保存行数、正反序、自动滚动三个偏好 |

`invalidate()` 只改变逻辑代际，**不清零尚未完成的物理请求计数**。这是本卡最不能简化的地方。

### 7.2 必测竞态矩阵

在 `LogCenterStore.test.ts` 中 mock `@tauri-apps/api/core` 的 invoke，使用可手动 resolve/reject 的 deferred promise。Settings、Lifecycle、SwitchGuard 用响应式 ref 对象或真实 Store，不使用不响应变化的普通常量。测试之间销毁 Pinia effect scope，清理 fake timers。

| 编号 | 固定场景 | 必须断言 |
|---|---|---|
| L01 | 未打开日志页推进 30 秒 | invoke 0 次 |
| L02 | 打开，返回 offset=100；下一次轮询 | 第一次 false/0，第二次 true/100 |
| L03 | A 请求未回，切 B，B 先成功，然后 A 成功 | 最终仅 B 内容、B offset；不出现 A |
| L04 | 同 L03，但 A 最后 reject | B 的 error 不变、不弹 A 的错误 |
| L05 | 同 id 修改服务器 URL | 清空，发送新 expectedServerUrl，全量 offset=0 |
| L06 | 同 id 修改用户名或密码 | 旧结果失效；invoke 参数不包含凭据 |
| L07 | 当前请求中关闭页面/退后台/暂停，再 resolve | 不写入、不产生下一轮 |
| L08 | 暂停/后台后恢复 | 第一轮 incremental=false |
| L09 | 快速刷新 20 次，前两个 promise 不完成 | 在途 invoke 最大 2；没有无限请求 |
| L10 | L09 中旧请求完成 | 只启动最新待执行代际，不补发 20 个旧任务 |
| L11 | 新请求进行中，旧 finally 执行 | isLoading 仍为 true |
| L12 | 返回半行“半”，下次“行\n” | 显示“半行”，不重复半行 |
| L13 | 增量 content 为空 | 不增加新行数徽标、不制造额外日志行 |
| L14 | 增量 needFullReload=true | 下一请求 false/0，旧缓冲和半行清空 |
| L15 | 全量连续返回 needFullReload=true | 不形成即时无限重试，进入退避 |
| L16 | 非安全整数/负 offset、缺失字段 | 可见错误，不污染已验证的缓冲 |
| L17 | 超长行及大量行连续追加 | 行数≤limit；单行≤65536 字符；总缓冲≤1048576 字符 |
| L18 | setLineLimit(100)，原有 500 行 | 只留最后 100 行，不改变服务器 offset |
| L19 | `$dispose()` 后旧 promise 完成 | 不新建 timer，不启动新 invoke |
| L20 | 线路切换失败，返回原配置 | 原缓冲已清，重新全量，不混用切换前响应 |
| L21 | 修改主题/排序等无关设置，四个连接字段不变 | 不清空日志、不新建请求代际 |

必须运行真实异步测试，不允许用“源码含 epoch”代替 L03/L04/L09/L11。

```bash
pnpm exec vitest run src/tests/unit/port/RequestEpoch.test.ts src/tests/unit/port/LogCenterStore.test.ts
pnpm exec vue-tsc --noEmit
```

## 8. T05 — 搬上游日志页面，只做指定删改

### 8.1 取件

从固定上游读取且只新增这个文件：

```text
src/features/logcenter/LogCenterView.vue
```

原始 Git blob 必须为 `8c081346613faef362e6ab7a9f9ed2646d9f07fe`。若目标已存在不同内容，停止核对，不能覆盖。可沿用 T01 助手，单独指定该文件及校验值；不要把它加入“交付仍须原样”的六文件表，因为后续有意修改。

### 8.2 指定修改清单

**A. 删除不用的 import 与实例。** 删除 `useOverlayStore` 的 import 和 `const overlayStore = useOverlayStore();`，其余依赖不替换。

**B. 加顶层可见参数。** 将 props 声明固定为：

```ts
const props = withDefaults(defineProps<{
  isOpen?: boolean;
  isActive?: boolean;
  zIndex?: number;
}>(), {
  isOpen: false,
  isActive: false,
  zIndex: 40,
});
```

会话生命周期 watch 的 source 从 `() => props.isOpen` 改为：

```ts
() => props.isOpen && props.isActive
```

开时仍调用 `store.startSession()`，关时仍调用 `store.stopSession()`。保留 `onBeforeUnmount` 的 `resetSession()`。

**C. 删所有清空入口。** 整段删除 `confirmClearServer()`。删除 menuActions 中“清空服务器日志…”和“清空本地显示”两个完整对象。删除后页面不能再引用 clearServer / clearLocal。不能用隐藏按钮或假函数替代真正删除。

**D. 只保留一个 BottomSheet。**

- 删除 `isLimitSheetOpen` 声明。
- 删除 `limitActions` 的 computed 声明。
- 删除 menuActions 中“行数限制”那个对象。
- 保留 `LIMIT_OPTIONS = [100, 300, 500, 1000, 3000]`。
- 删除行数限制的第二个 BottomSheet。
- 第一个 BottomSheet 保留 `v-model="isMenuOpen" title="日志选项" :actions="menuActions"`，去掉 `compact`。
- 不修改项目原有的 BottomSheet.vue。

在 `const isMenuOpen = ref(false);` 后增加：

```ts
watch(() => props.isOpen && props.isActive, (active) => {
  if (!active) isMenuOpen.value = false;
});

function onLineLimitChange(event: Event): void {
  const target = event.target;
  if (!(target instanceof HTMLSelectElement)) return;
  const limit = Number(target.value);
  if (LIMIT_OPTIONS.includes(limit)) store.setLineLimit(limit);
}
```

**E. 行数选择使用原生 select。** 在模板 `.log-toolbar` 中，搜索框后、正反序按钮前插入：

```vue
<select
  class="log-limit-select"
  aria-label="日志行数上限"
  :value="store.lineLimit"
  @change="onLineLimitChange"
>
  <option v-for="limit in LIMIT_OPTIONS" :key="limit" :value="limit">
    {{ limit }} 行
  </option>
</select>
```

在本文件 scoped CSS 末尾新增：

```css
.log-limit-select {
  min-height: 36px;
  max-width: 92px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  padding: 0 6px;
  background: var(--secondary-bg);
  color: var(--primary-text);
  font-size: 12px;
  flex-shrink: 0;
}
```

**F. 刷新按钮显式受当前状态控制。** 将本页 RefreshButton 改成：

```vue
<RefreshButton
  label="全量刷新"
  :loading="store.isLoading"
  :disabled="store.requestBlocked"
  @refresh="store.refresh()"
/>
```

在标题区 `日志中心` 后增加：

```vue
<span class="text-[10px] opacity-60">{{ store.sourceLabel }} · 只读</span>
```

**G. 复制只报告真实结果。** 保留 Clipboard API 首选；旧 execCommand 降级中必须检查布尔值，把 `document.execCommand('copy');` 改为：

```ts
if (!document.execCommand('copy')) throw new Error('copy failed');
```

失败不能显示“复制成功”。不额外接 Android 原生剪贴板或权限。

**H. 严格保持上游展示算法。** 保留 useVirtualList、22px 行高、overscan=15、数据收缩后的虚拟窗口校正、日志级别展示、文本节点高亮。不要替换成完整 v-for，不引入 v-html，不把日志交给 Markdown 渲染器。

### 8.3 本卡检查

```bash
pnpm exec vue-tsc --noEmit
pnpm build
```

`LogCenterIntegration.test.ts` 验证：页面只有一个 BottomSheet；没有 compact/clear 操作；原生 select 改行数；刷新按钮实际调用适配版 Store；后台/非顶层页不会启动新拉取。

## 9. T06/T07/T08 — 接入口、总验收与交付

### T06-a：只扩展 Overlay Store，不重写它

文件：`src/core/stores/overlay.ts`。

在现有 isGlobalSearchOpen 附近新增：

```ts
const isLogCenterOpen = computed(() => pageStack.value.some(p => p.type === 'logCenter'));
const isLogCenterActive = computed(() => pageStackTop.value?.type === 'logCenter');
```

在 openGlobalSearch/closeGlobalSearch 附近新增：

```ts
const openLogCenter = () => {
  if (!isLogCenterOpen.value) pushPage('logCenter');
};
const closeLogCenter = () => {
  if (pageStackTop.value?.type === 'logCenter') popPage();
};
```

在 return 对象中导出这四个名称。**不要改 pageStack、ModalHistory、已有 open/close 的实现。** 打开状态与顶层活动状态分开，是为了另一个业务页盖住日志时暂停拉取。

### T06-b：懒加载并保留离场动画

文件：`src/components/FeatureOverlays.vue`。

1. 从 vue 的现有 import 增加 `watch`。
2. 按已有 defineAsyncComponent 风格增加：

```ts
const LogCenterView = defineAsyncComponent(() => import('../features/logcenter/LogCenterView.vue'));
```

在 overlayStore 初始化后增加：

```ts
const logCenterMounted = ref(false);
watch(() => overlayStore.isLogCenterOpen, (open) => {
  if (open) logCenterMounted.value = true;
}, { immediate: true });
```

在现有页面后增加：

```vue
<LogCenterView
  v-if="logCenterMounted"
  :is-open="overlayStore.isLogCenterOpen"
  :is-active="overlayStore.isLogCenterActive"
  :z-index="overlayStore.getPageZIndex('logCenter')"
  @close="overlayStore.closeLogCenter()"
/>
```

不把页面加入应用启动预加载；不使用 `v-if="overlayStore.isLogCenterOpen"` 直接砍掉离场组件；也不让每次打开重新注册一组全局监听器。

### T06-c：在右侧工具区添加一个按钮

文件：`src/components/layout/RightSidebar.vue`。

lucide-vue-next import 增加 `FileText`。在已有 openRagObserverView 附近新增：

```ts
const openLogCenterView = () => {
  overlayStore.openLogCenter();
  emit('close');
};
```

在现有 `grid grid-cols-2 gap-2` 内的最后一个按钮之后加入：

```vue
<button
  type="button"
  class="col-span-2 py-3 px-4 rounded-full transition-all text-primary-text flex items-center justify-center gap-2 active:scale-95 border border-black/5 dark:border-white/5 bg-black/5 dark:bg-white/5"
  @click="openLogCenterView"
>
  <FileText :size="15" />
  <span class="font-bold text-[11px] leading-none">日志中心 · 只读</span>
</button>
```

既有插件中心、线路切换、DailyNote、RAG 四个按钮必须仍在，绑定函数不得改。无需为了同步上游“更多托盘”重做这个抽屉。

### T07：集成门禁

按以下顺序逐条运行；每条写独立结果：

```bash
pnpm exec vitest run src/tests/unit/port
pnpm exec vue-tsc --noEmit
pnpm test:run
pnpm build
cargo check --manifest-path src-tauri/Cargo.toml --locked
cargo test --manifest-path src-tauri/Cargo.toml --locked --lib
cargo fmt --all --manifest-path src-tauri/Cargo.toml -- --check
```

随后设置 `FINAL = True`，运行附录 E 的白名单、全部交付物存在性和精确源文件检查。追加检查：

```bash
git diff --check bae00da0e3669de9592c838f1b7ff4592591c78d
git diff --stat bae00da0e3669de9592c838f1b7ff4592591c78d
```

不要修改 package.json 加快捷命令；不要加新 lint 配置；不要因为上游 Formatter 版本不同格式化整个 fork。

集成测试最少额外断言：

| 场景 | 预期 |
|---|---|
| 右栏点击日志入口 | 日志页打开，右栏关闭，只有一个 logCenter 栈项 |
| 快速重复点击入口 | 不重复压栈 |
| Android 返回键 | 先按原 ModalHistory 关闭菜单，再关闭日志页；不影响其他页面 |
| 打开另一个业务页覆盖日志页 | isLogCenterOpen=true，但 isLogCenterActive=false，不再轮询 |
| 从覆盖页返回日志页 | 重新全量，然后增量 |
| 日志页首次异步加载期间立刻关闭 | 组件晚加载后不发送请求 |
| 日志中有 `<script>`、HTML 标签、ANSI | 当普通日志文字显示，不执行 HTML |
| 3000 行→100 行 | 虚拟窗口恢复，不出现空白区域 |
| 切线路 | 顶部来源文字切换，旧数据不会混入 |
| 数据库未就绪 | 新日志命令仍受原启动门禁拦截，不加入启动白名单 |

### T08：APK 与真机验证

使用 fork 已有脚本，不改构建流程：

```bash
pnpm android:build:phone
pnpm android:verify:phone
```

缺 Android SDK/NDK/Java 等环境，只写 `BLOCKED_ENV`，不能改版本或回退签名校验。不自动安装 APK。通过用户明确指定的测试设备/测试应用执行以下人工验证，不覆盖用户唯一的在用实例。

| 人工检查 | 操作与预期 |
|---|---|
| 两个测试服务器 | A/B 返回不同标记日志，重复切换只看到当前来源 |
| 没有管理接口的测试服务器 | 显示不支持/文件不存在提示；不要求改 Wire 或 DB |
| 错误管理员凭据 | 显示鉴权错误，不自动降级、无真实清空请求 |
| 前后台与暂停 | 观察测试服务请求计数，不再启动新轮询；在途旧请求可能结束，但不回写 |
| 快速刷新与关页 | 不闪回旧日志，不持续产生无限请求 |
| 视觉 | 删除徽标完整、群成员限高滚动、RAG 收起定位、按钮底色正常 |
| 回归 | 聊天、附件发送、搜索、连接切换、DailyNote 原功能仍能使用 |
| 只读边界 | 日志中心没有清空入口；测试服务器没有收到 POST /server-log/clear |

没有测试设备或测试服务器，记 `NOT_RUN`，交付状态只能是“代码检查通过、运行验收待完成”。不能写“真机可用”。

### 9.1 本地提交与远端边界

只有该阶段检查通过，才允许在工作分支做本地小提交。建议拆为：独立组件/工具、四处 UI 修复、只读日志后端、隔离 Store、页面与接线、最终测试。暂时不要 push、创建 PR、合并 main 或发 Release。

提交前列明具体文件后 `git add -- <文件...>`，不要无脑 `git add .`。本轮授权是编写施工图；这些提交动作属于未来执行者在用户允许实施时的步骤，本文编制过程没有执行它们。

## 10. 唯一执行台账

状态只能填写：`TODO`、`DOING`、`CODE_DONE`、`VERIFIED`、`BLOCKED`。每条验证另填 `PASS / FAIL / NOT_RUN / BLOCKED_ENV`。CODE_DONE 不能冒充 VERIFIED。

| 任务 | 状态 | 实际变更/本地提交 | 执行过的命令及退出码 | 未完成或阻塞 |
|---|---|---|---|---|
| T00 基线 | VERIFIED | 从本地 `main@bae00da0` 创建 `port/upstream-low-risk-20260909`；固定上游对象已抓取；未提交 | `node --version` 0；`pnpm --version` 0；`cargo --version` 0；`rustc --version` 0；`java -version` 0；`pnpm install --frozen-lockfile` 0；`pnpm exec vue-tsc --noEmit` 0；`pnpm test:run` 0（210/210）；`cargo check --manifest-path src-tauri/Cargo.toml --locked` 0；`cargo test --manifest-path src-tauri/Cargo.toml --locked --lib` 0（579 passed，1 ignored） | 施工图原 BASE 为 `793ec44f`；依用户指令以其后继本地 `main@bae00da0` 为 fork 起点，未回退 |
| T01 五个独立文件 | VERIFIED | 原样新增 5 个固定来源文件；注册 `http_clients`；新增 2 个端口测试文件；未提交 | `pnpm exec vitest run src/tests/unit/port/DirectAssets.test.ts src/tests/unit/port/RefreshButton.test.ts` 0（12/12）；`pnpm exec vue-tsc --noEmit` 0；`cargo test --manifest-path src-tauri/Cargo.toml --locked --lib http_clients` 0（3/3） | 5 个 blob SHA 全部匹配；无阻塞 |
| T02-a 样式入口 | VERIFIED | 仅交换 UnoCSS reset 与生成样式导入顺序；未提交 | T02 共享门禁：`pnpm exec vitest run src/tests/unit/port/LowRiskUi.test.ts` 0（3/3）；`pnpm exec vue-tsc --noEmit` 0；`pnpm build` 0；`git diff --check` 0 | 无阻塞 |
| T02-b 附件徽标 | VERIFIED | 仅移除附件预览最外层 `overflow-hidden`；删除事件 index 行为测试通过；未提交 | T02 共享门禁：Vitest 0；vue-tsc 0；build 0；diff check 0 | 无阻塞 |
| T02-c RAG 定位 | VERIFIED | 两个收起定位改为 `instant`，Tab 定位仍为 `smooth`；未提交 | T02 共享门禁：Vitest 0；vue-tsc 0；build 0；diff check 0 | 无阻塞 |
| T02-d 群成员限高 | VERIFIED | 成员区增加 `max-h-80`、纵向滚动与现有滚动样式；toggle agentId 行为测试通过；未提交 | T02 共享门禁：Vitest 0；vue-tsc 0；build 0；diff check 0 | 无阻塞 |
| T03 日志后端 | VERIFIED | 原样新增 `admin_api.rs`；新增只读 `log_service.rs` 与模块/handler 注册；未提交 | `cargo check --manifest-path src-tauri/Cargo.toml --locked` 0；`cargo test --manifest-path src-tauri/Cargo.toml --locked --lib logcenter` 0（7/7）；`cargo test --manifest-path src-tauri/Cargo.toml --locked --lib admin_api` 0（7/7）；`git diff --check` 0 | `admin_api.rs` blob 匹配；日志命令仅注册 `logcenter_fetch`，无清空命令；无阻塞 |
| T04 隔离 Store | VERIFIED | 完整加入附录 B/C/D；新增 L01–L21 响应式竞态/边界测试及跨响应 ANSI 清理回归；未提交 | `pnpm exec vitest run src/tests/unit/port/RequestEpoch.test.ts src/tests/unit/port/LogCenterStore.test.ts` 0（31/31）；`pnpm exec vue-tsc --noEmit` 0；`git diff --check` 0 | 已验证双在途上限、旧响应隔离、连接/凭据变化、跨响应 ANSI 清理、完整 6/12/24/30 秒退避、70,000 字符长行与总缓冲上限；无阻塞 |
| T05 日志页面 | VERIFIED | 固定上游页面取件后完成 A–H 只读适配；新增页面集成测试；未提交 | `pnpm exec vitest run src/tests/unit/port/LogCenterIntegration.test.ts` 0（4/4）；`pnpm exec vue-tsc --noEmit` 0；`pnpm build` 0（4588 modules）；`git diff --check` 0 | 仅一个 BottomSheet；无 clear/compact；保留虚拟列表与文本节点高亮；无阻塞 |
| T06 导航接线 | VERIFIED | 扩展 Overlay Store；懒加载且一次挂载日志页；新增右栏只读入口；扩展 10 个导航/集成场景；未提交 | `pnpm exec vitest run src/tests/unit/port/LogCenterIntegration.test.ts` 0（14/14）；`pnpm exec vitest run src/tests/unit/port` 0（59/59）；`pnpm exec vue-tsc --noEmit` 0；`pnpm build` 0；`git diff --check` 0 | 入口不重复压栈，open/active 分离，返回栈和异步关闭通过；无阻塞 |
| T07 集成验证 | VERIFIED | 全量前端/Rust 回归与最终范围审计完成；CI 格式回归已修正；未提交 | `pnpm exec vitest run src/tests/unit/port` 0（60/60）；`pnpm exec vue-tsc --noEmit` 0；`pnpm test:unit` 0（270/270）；`pnpm build` 0（4600 modules）；`cargo fmt --all -- --check` 0；`cargo check --manifest-path src-tauri/Cargo.toml --locked` 0；`cargo test --locked --workspace --lib` 0（插件 8/8，主 crate 596 passed、1 ignored）；附录 E `FINAL=True` 等价检查 PASS；`git diff --check bae00da0` 0 | 28/28 变更路径在白名单；交付物齐全；6 个原样 blob 匹配；无阻塞 |
| T08 APK/真机 | CODE_DONE | 生成并校验 `app-universal-release.apk`（30,692,934 bytes，arm64-v8a，Debug 证书）；未安装、未提交 | `pnpm android:build:phone` 首次受限网络运行 1（BLOCKED_ENV：esm.sh）；受控联网重跑 0（Rust release、Gradle、v2 签名、zipalign、ELF 16KB 页对齐 PASS）；`pnpm android:verify:phone` 0（ABI/ELF 16KB PASS）；`adb devices -l` 0（无设备） | 真机、双测试服务器、错误凭据与回归人工验收 `NOT_RUN`；不得宣称真机可用 |

遇到阻塞，只在本节追加这一种记录，不新建日志文档：

```text
任务：Txx
失败步骤：
预期锚点/API/测试结果：
实际情况：
已经修改的白名单文件：
已运行的检查及退出码：
未执行的检查：
没有执行：扩大文件范围 / 修改锁文件 / 放宽权限 / 删除测试 / push
```

日志正文、Authorization、API Key、密码、真实数据库内容不得写进本台账。错误只摘录定位所需的最小信息，并先脱敏。

### 10.1 完成定义

本轮全部完成要求：原样文件校验通过，白名单无越界，基线与新增测试通过，旧测试不回归，Rust/前端/APK 构建通过，测试设备的连接隔离和只读行为验收通过。

允许诚实交付部分完成；不允许为了完成状态绕过门禁。最终特别注明：Wire 仍是原 fork 实现；没有升级数据库、渲染引擎或 OTA；没有新增论坛、邮箱、任务中心或完整群聊邀请。

本次交付结论（2026-09-09）：代码、前端/Rust 回归与 APK 构建检查均通过；因无连接设备和测试服务器，T08 人工验收保持 `NOT_RUN`。Wire 仍为原 fork 实现；未升级数据库、渲染引擎或 OTA；未新增论坛、邮箱、任务中心或完整群聊邀请。日志中心未注册任何清空命令，实际业务请求仅使用 GET。台账各卡中的“未提交”记录对应卡级检查时点；全部白名单变更随后统一纳入本轮最终本地提交。

## 11. 源码证据与范围说明

仓库事实按下列固定来源核对；任务范围、只读裁剪、缓存策略与适配代码是本施工图的设计决定，不声称是上游原有实现。

- [E01 fork 冻结起点](https://github.com/awei807-wei/VCPMobile/commit/793ec44f405a2f4f1529629dbc8096cf56750812)
- [E02 上游冻结起点](https://github.com/MRiecy/VCPMobile/commit/4d53b548572ac4d34c4c69f83188605a24bf99e2)
- [E03 fork 样式入口](https://github.com/awei807-wei/VCPMobile/blob/793ec44f405a2f4f1529629dbc8096cf56750812/src/main.ts)
- [E04 fork 群成员拆分](https://github.com/awei807-wei/VCPMobile/blob/793ec44f405a2f4f1529629dbc8096cf56750812/src/features/agent/GroupMembersSection.vue)
- [E05 fork 连接切换门禁](https://github.com/awei807-wei/VCPMobile/blob/793ec44f405a2f4f1529629dbc8096cf56750812/src/core/stores/connectionSwitchGuard.ts)
- [E06 fork 连接配置切换流程](https://github.com/awei807-wei/VCPMobile/blob/793ec44f405a2f4f1529629dbc8096cf56750812/src/core/stores/connectionProfiles.ts)
- [E07 fork Settings 和 read_settings](https://github.com/awei807-wei/VCPMobile/blob/793ec44f405a2f4f1529629dbc8096cf56750812/src-tauri/src/vcp_modules/infra/settings_manager.rs)
- [E08 fork BottomSheet](https://github.com/awei807-wei/VCPMobile/blob/793ec44f405a2f4f1529629dbc8096cf56750812/src/components/ui/BottomSheet.vue)
- [E09 上游日志服务原始实现](https://github.com/MRiecy/VCPMobile/blob/4d53b548572ac4d34c4c69f83188605a24bf99e2/src-tauri/src/vcp_modules/logcenter/log_service.rs)
- [E10 上游日志 Store 原始实现](https://github.com/MRiecy/VCPMobile/blob/4d53b548572ac4d34c4c69f83188605a24bf99e2/src/features/logcenter/logCenterStore.ts)
- [E11 上游日志页面原始实现](https://github.com/MRiecy/VCPMobile/blob/4d53b548572ac4d34c4c69f83188605a24bf99e2/src/features/logcenter/LogCenterView.vue)
- [E12 Git 固定来源恢复指定文件](https://git-scm.com/docs/git-restore)
- [E13 fork 命令注册与中央 handler](https://github.com/awei807-wei/VCPMobile/blob/793ec44f405a2f4f1529629dbc8096cf56750812/src-tauri/src/lib.rs)
- [E14 fork 启动门禁](https://github.com/awei807-wei/VCPMobile/blob/793ec44f405a2f4f1529629dbc8096cf56750812/src-tauri/src/vcp_modules/infra/invoke_guard.rs)
- [E15 fork 原有测试/构建命令](https://github.com/awei807-wei/VCPMobile/blob/793ec44f405a2f4f1529629dbc8096cf56750812/package.json)

原样文件各自的永久来源：

- [src/components/ui/RefreshButton.vue](https://github.com/MRiecy/VCPMobile/blob/4d53b548572ac4d34c4c69f83188605a24bf99e2/src/components/ui/RefreshButton.vue)
- [src/core/utils/nameHue.ts](https://github.com/MRiecy/VCPMobile/blob/4d53b548572ac4d34c4c69f83188605a24bf99e2/src/core/utils/nameHue.ts)
- [src/core/utils/mention.ts](https://github.com/MRiecy/VCPMobile/blob/4d53b548572ac4d34c4c69f83188605a24bf99e2/src/core/utils/mention.ts)
- [src/features/logcenter/logText.ts](https://github.com/MRiecy/VCPMobile/blob/4d53b548572ac4d34c4c69f83188605a24bf99e2/src/features/logcenter/logText.ts)
- [src-tauri/src/vcp_modules/infra/http_clients.rs](https://github.com/MRiecy/VCPMobile/blob/4d53b548572ac4d34c4c69f83188605a24bf99e2/src-tauri/src/vcp_modules/infra/http_clients.rs)
- [src-tauri/src/vcp_modules/infra/admin_api.rs](https://github.com/MRiecy/VCPMobile/blob/4d53b548572ac4d34c4c69f83188605a24bf99e2/src-tauri/src/vcp_modules/infra/admin_api.rs)

本地补丁参考提交：`22f4d3df148fbd4cdb1b06f7eb5b47c696161f3e`（样式顺序）、`b17ebf75e88fefae80295340c359883850fe2fd0`（附件徽标）、`23b41ff723910970b04a6331cce1cbcc81cb7730`（RAG 定位）、`3a8f528e2888ebe4219bb86997ac9069528234e2`（成员列表）。只引用改动意图，不整条 cherry-pick。

## 附录 A — 新增 log_service.rs 的完整内容

目标：`src-tauri/src/vcp_modules/logcenter/log_service.rs`。这是 fork 适配版，不是 COPY_EXACT。

```rust
//! Fork read-only adaptation of upstream logcenter/log_service.rs.
//! Commands accept an expected connection identity, never caller-controlled credentials.
use crate::vcp_modules::infra::admin_api;
use crate::vcp_modules::settings_manager::{
    is_connection_profile_switching, read_settings, Settings, SettingsState,
};
use futures_util::StreamExt;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Runtime, State};

const FETCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_JS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFetchResult {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub offset: u64,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub file_size: u64,
    #[serde(default)]
    pub need_full_reload: bool,
}

fn validate_connection_scope(
    settings: &Settings,
    expected_profile_id: &str,
    expected_server_url: &str,
) -> Result<(), String> {
    if settings.active_connection_profile_id != expected_profile_id {
        return Err("CONNECTION_CHANGED: 线路已变化，请重新加载日志".to_string());
    }
    let actual = admin_api::normalize_server_base(&settings.vcp_server_url)?;
    let expected = admin_api::normalize_server_base(expected_server_url)?;
    if actual != expected {
        return Err("CONNECTION_CHANGED: 服务器地址已变化，请重新加载日志".to_string());
    }
    Ok(())
}

fn append_capped_bytes(buffer: &mut Vec<u8>, chunk: &[u8]) -> Result<(), String> {
    if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(buffer.len()) {
        return Err("日志响应超过 8 MiB，已停止读取".to_string());
    }
    buffer.extend_from_slice(chunk);
    Ok(())
}

async fn read_capped_text(resp: reqwest::Response) -> Result<String, String> {
    if resp.content_length().is_some_and(|len| len > MAX_RESPONSE_BYTES as u64) {
        return Err("日志响应超过 8 MiB，已拒绝读取".to_string());
    }
    let mut buffer = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "日志响应读取失败".to_string())?;
        append_capped_bytes(&mut buffer, &chunk)?;
    }
    String::from_utf8(buffer).map_err(|_| "服务器返回了非 UTF-8 响应".to_string())
}

#[tauri::command]
pub async fn logcenter_fetch<R: Runtime>(
    app_handle: AppHandle<R>,
    settings_state: State<'_, SettingsState>,
    incremental: bool,
    offset: u64,
    expected_profile_id: String,
    expected_server_url: String,
) -> Result<LogFetchResult, String> {
    if offset > MAX_JS_SAFE_INTEGER {
        return Err("日志游标超出安全范围".to_string());
    }
    if is_connection_profile_switching(&app_handle) {
        return Err("CONNECTION_SWITCHING: 正在切换线路".to_string());
    }
    let settings = read_settings(app_handle.clone(), settings_state).await?;
    admin_api::ensure_admin_config(&settings, "日志中心")?;
    validate_connection_scope(&settings, &expected_profile_id, &expected_server_url)?;
    if is_connection_profile_switching(&app_handle) {
        return Err("CONNECTION_SWITCHING: 正在切换线路".to_string());
    }

    // The target is derived ONLY from the backend Settings snapshot.
    let mut request = admin_api::admin_request(&settings, Method::GET, &["server-log"])?;
    if incremental {
        request = request.query(&[("incremental", "true".to_string()), ("offset", offset.to_string())]);
    }
    let resp = request.timeout(FETCH_TOTAL_TIMEOUT).send().await
        .map_err(|_| "日志拉取失败，请检查连接或稍后重试".to_string())?;
    match resp.status() {
        StatusCode::OK => {}
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN =>
            return Err("管理员凭据校验失败，请检查当前配置".to_string()),
        StatusCode::NOT_FOUND =>
            return Err("日志文件不存在，或服务器不支持日志接口".to_string()),
        StatusCode::SERVICE_UNAVAILABLE =>
            return Err("日志服务暂不可用，请稍后重试".to_string()),
        status => return Err(format!("日志拉取失败: HTTP {}", status.as_u16())),
    }
    let body = read_capped_text(resp).await?;
    let mut result: LogFetchResult = serde_json::from_str(&body)
        .map_err(|_| "服务器日志响应不符合 JSON 契约".to_string())?;
    if result.offset > MAX_JS_SAFE_INTEGER || result.file_size > MAX_JS_SAFE_INTEGER {
        return Err("日志响应中的大小或游标超出安全范围".to_string());
    }
    // Retain upstream's boundary replacement-character behavior.
    result.content = result.content.trim_matches('\u{FFFD}').to_string();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            active_connection_profile_id: "lan".to_string(),
            vcp_server_url: "https://example.test/vcp/v1/chat/completions".to_string(),
            admin_username: "test-admin".to_string(),
            admin_password: "not-a-real-secret".to_string(),
            ..Default::default()
        }
    }
    #[test]
    fn same_profile_and_normalized_server_are_accepted() {
        assert!(validate_connection_scope(&settings(), "lan", "https://example.test/vcp/").is_ok());
    }
    #[test]
    fn different_profile_is_rejected() {
        assert!(validate_connection_scope(&settings(), "wan", "https://example.test/vcp/").is_err());
    }
    #[test]
    fn different_server_is_rejected() {
        assert!(validate_connection_scope(&settings(), "lan", "https://other.test/vcp/").is_err());
    }
    #[test]
    fn embedded_credentials_are_rejected() {
        assert!(validate_connection_scope(&settings(), "lan", "https://user:pass@example.test/vcp/").is_err());
    }
    #[test]
    fn oversized_chunk_is_not_appended() {
        let mut bytes = vec![0; MAX_RESPONSE_BYTES - 1];
        assert!(append_capped_bytes(&mut bytes, &[1, 2]).is_err());
        assert_eq!(bytes.len(), MAX_RESPONSE_BYTES - 1);
    }
    #[test]
    fn utf8_split_across_chunks_is_preserved() {
        let source = "中文日志".as_bytes();
        let mut bytes = vec![];
        append_capped_bytes(&mut bytes, &source[..2]).unwrap();
        append_capped_bytes(&mut bytes, &source[2..]).unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "中文日志");
    }
    #[test]
    fn rotation_notice_has_safe_defaults() {
        let result: LogFetchResult = serde_json::from_str(r#"{"needFullReload":true,"offset":0}"#).unwrap();
        assert!(result.need_full_reload);
        assert_eq!(result.file_size, 0);
    }
}

```

## 附录 B — 新增 requestEpoch.ts 的完整内容

目标：`src/features/logcenter/requestEpoch.ts`。不要删除在途上限，不要把 invalidate 改成清空 active。

```ts
/** Only local request bookkeeping. invalidate() does NOT cancel an HTTP request. */
export interface RequestTicket {
  readonly epoch: number;
  readonly id: number;
}

export class RequestEpoch {
  private epoch = 0;
  private sequence = 0;
  private readonly active = new Map<number, number>();

  invalidate(): void {
    this.epoch += 1;
  }

  get inFlight(): number {
    return this.active.size;
  }

  get currentInFlight(): number {
    let count = 0;
    for (const epoch of this.active.values()) {
      if (epoch === this.epoch) count += 1;
    }
    return count;
  }

  begin(): RequestTicket | null {
    // One request per epoch; at most two physical invokes including obsolete ones.
    if (this.active.size >= 2 || this.currentInFlight > 0) return null;
    const ticket = Object.freeze({ epoch: this.epoch, id: ++this.sequence });
    this.active.set(ticket.id, ticket.epoch);
    return ticket;
  }

  isCurrent(ticket: RequestTicket): boolean {
    return ticket.epoch === this.epoch && this.active.get(ticket.id) === ticket.epoch;
  }

  finish(ticket: RequestTicket): void {
    // An obsolete finally can release only its own request, never the newer one.
    if (this.active.get(ticket.id) === ticket.epoch) this.active.delete(ticket.id);
  }
}

```

## 附录 C — 新增 logCenterStore.ts 的完整内容

目标：`src/features/logcenter/logCenterStore.ts`。不注册远端清空命令，不保存日志或凭据，只保存三个显示偏好。源码中的超时等待由 Rust 的请求总超时收束，不通过伪装前端取消释放在途计数。

```ts
// Fork adaptation: read-only log viewer. No remote clear command and no log persistence.
import { defineStore } from 'pinia';
import { computed, onScopeDispose, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useSettingsStore } from '../../core/stores/settings';
import { useAppLifecycleStore } from '../../core/stores/appLifecycle';
import { useConnectionSwitchGuardStore } from '../../core/stores/connectionSwitchGuard';
import { RequestEpoch } from './requestEpoch';
import { LINE_LIMIT_DEFAULT, clampLineLimit, splitLogChunk, stripAnsi } from './logText';

interface LogFetchResult {
  content: string;
  offset: number;
  path: string;
  fileSize: number;
  needFullReload: boolean;
}

type ScopeSnapshot = readonly [string, string, string, string];
const POLL_MS = 3000;
const MAX_BACKOFF_MS = 30000;
const MAX_LINE_CHARS = 65536;
const MAX_BUFFER_CHARS = 1048576;
const OMITTED = '[前部日志已省略] ';
const KEYS = {
  limit: 'vcp_log_limit',
  reverse: 'vcp_log_reverse',
  autoScroll: 'vcp_log_autoscroll',
} as const;

function readPreference(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}
function writePreference(key: string, value: string): void {
  try { localStorage.setItem(key, value); } catch { /* Preferences only. */ }
}
function clipLine(line: string): string {
  if (line.length <= MAX_LINE_CHARS) return line;
  return OMITTED + line.slice(-(MAX_LINE_CHARS - OMITTED.length));
}
function boundedLines(input: string[], limit: number): string[] {
  const result = input.slice(-limit).map(clipLine);
  let chars = result.reduce((sum, line) => sum + line.length, 0);
  let first = 0;
  while (chars > MAX_BUFFER_CHARS && first < result.length - 1) {
    chars -= result[first++].length;
  }
  return result.slice(first);
}
function validResult(value: unknown): value is LogFetchResult {
  if (!value || typeof value !== 'object') return false;
  const v = value as Record<string, unknown>;
  return typeof v.content === 'string' && typeof v.path === 'string'
    && typeof v.needFullReload === 'boolean'
    && typeof v.offset === 'number' && Number.isSafeInteger(v.offset) && v.offset >= 0
    && typeof v.fileSize === 'number' && Number.isSafeInteger(v.fileSize) && v.fileSize >= 0;
}

export const useLogCenterStore = defineStore('logCenter', () => {
  const settingsStore = useSettingsStore();
  const lifecycleStore = useAppLifecycleStore();
  const switchGuard = useConnectionSwitchGuardStore();
  const requests = new RequestEpoch();
  const lines = ref<string[]>([]);
  const pendingFragment = ref('');
  const offset = ref(0);
  const logPath = ref('');
  const fileSize = ref(0);
  const hasSnapshot = ref(false);
  const sessionActive = ref(false);
  const isPaused = ref(false);
  const isLoading = ref(false);
  const error = ref<string | null>(null);
  const consecutiveFailures = ref(0);
  const newLineCount = ref(0);
  const logVersion = ref(0);
  const filterText = ref('');
  const lineLimit = ref(clampLineLimit(Number(readPreference(KEYS.limit)) || LINE_LIMIT_DEFAULT));
  const isReverse = ref(readPreference(KEYS.reverse) === '1');
  const autoScroll = ref(readPreference(KEYS.autoScroll) !== '0');
  let timer: ReturnType<typeof setTimeout> | null = null;
  let forceFull = true;
  let needsRun = false;
  let microtaskQueued = false;
  let disposed = false;

  // Credentials are compared in memory only. Never log, persist, or send this tuple.
  function scopeSnapshot(): ScopeSnapshot {
    const settings = settingsStore.settings;
    return [
      settings?.activeConnectionProfileId || 'lan',
      settings?.vcpServerUrl || '',
      settings?.adminUsername || '',
      settings?.adminPassword || '',
    ];
  }
  function sameScope(snapshot: ScopeSnapshot): boolean {
    const now = scopeSnapshot();
    return snapshot.every((item, index) => item === now[index]);
  }
  const canRun = computed(() => sessionActive.value && !isPaused.value
    && !lifecycleStore.isBackground && !switchGuard.switching && !!settingsStore.settings);
  const requestBlocked = computed(() => !canRun.value);
  const isPolling = computed(() => canRun.value);
  const sourceLabel = computed(() => scopeSnapshot()[0] === 'wan' ? '外网' : '内网');
  const matchedLines = computed(() => {
    const keyword = filterText.value.trim().toLowerCase();
    return keyword ? lines.value.filter(line => line.toLowerCase().includes(keyword)) : lines.value;
  });
  const displayedLines = computed(() => isReverse.value ? [...matchedLines.value].reverse() : matchedLines.value);
  const totalBuffered = computed(() => lines.value.length);
  const matchedCount = computed(() => matchedLines.value.length);

  function clearTimer(): void {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  }
  function clearBuffer(): void {
    lines.value = [];
    pendingFragment.value = '';
    offset.value = 0;
    logPath.value = '';
    fileSize.value = 0;
    hasSnapshot.value = false;
    newLineCount.value = 0;
    logVersion.value += 1;
  }
  function invalidate(clear: boolean): void {
    requests.invalidate();
    clearTimer();
    needsRun = false;
    forceFull = true;
    isLoading.value = false;
    error.value = null;
    consecutiveFailures.value = 0;
    if (clear) clearBuffer();
    // Do not reset requests.inFlight: obsolete native invokes still exist.
  }
  function queuePump(): void {
    if (disposed) return;
    needsRun = true;
    clearTimer();
    if (microtaskQueued) return;
    microtaskQueued = true;
    queueMicrotask(() => {
      microtaskQueued = false;
      void pump();
    });
  }
  function scheduleNext(): void {
    if (disposed || !canRun.value || timer !== null || requests.currentInFlight > 0) return;
    const delay = Math.min(MAX_BACKOFF_MS, POLL_MS * 2 ** Math.min(consecutiveFailures.value, 4));
    timer = setTimeout(() => { timer = null; queuePump(); }, delay);
  }
  function applyResult(data: LogFetchResult, incremental: boolean): void {
    if (incremental && !data.content) {
      offset.value = data.offset;
      logPath.value = data.path;
      fileSize.value = data.fileSize;
      return;
    }
    const carry = incremental ? pendingFragment.value : '';
    // ANSI 序列可能跨两次增量响应，必须先拼接上次半行再统一清理。
    const chunk = splitLogChunk(stripAnsi(`${carry}${data.content}`));
    const base = incremental
      ? (pendingFragment.value && lines.value.length ? lines.value.slice(0, -1) : lines.value)
      : [];
    lines.value = boundedLines([...base, ...chunk.lines], lineLimit.value);
    pendingFragment.value = clipLine(chunk.trailing);
    offset.value = data.offset;
    logPath.value = data.path;
    fileSize.value = data.fileSize;
    hasSnapshot.value = true;
    forceFull = false;
    newLineCount.value = incremental ? newLineCount.value + chunk.lines.length : 0;
    logVersion.value += 1;
  }
  async function pump(): Promise<void> {
    if (disposed || !canRun.value || !needsRun) return;
    const ticket = requests.begin();
    if (!ticket) return; // A finishing request will wake the newest pending attempt.
    needsRun = false;
    isLoading.value = true;
    const snapshot = scopeSnapshot();
    const incremental = hasSnapshot.value && !forceFull;
    const requestOffset = incremental ? offset.value : 0;
    try {
      const data: unknown = await invoke('logcenter_fetch', {
        incremental,
        offset: requestOffset,
        expectedProfileId: snapshot[0],
        expectedServerUrl: snapshot[1],
      });
      if (disposed || !requests.isCurrent(ticket) || !canRun.value || !sameScope(snapshot)) return;
      if (!validResult(data)) throw new Error('服务器日志响应不符合约定');
      if (data.needFullReload) {
        clearBuffer();
        forceFull = true;
        if (incremental) {
          needsRun = true; // Exactly one immediate full retry after rotation.
        } else {
          throw new Error('服务器全量响应仍要求重载，请稍后重试');
        }
      } else {
        if (incremental && data.offset < requestOffset) {
          forceFull = true;
          throw new Error('服务器日志游标回退，下次将全量重载');
        }
        applyResult(data, incremental);
      }
      error.value = null;
      consecutiveFailures.value = 0;
    } catch (cause: unknown) {
      if (!disposed && requests.isCurrent(ticket) && canRun.value && sameScope(snapshot)) {
        error.value = (cause instanceof Error ? cause.message : String(cause)).slice(0, 300);
        consecutiveFailures.value += 1;
      }
    } finally {
      requests.finish(ticket);
      if (!disposed) isLoading.value = requests.currentInFlight > 0;
      if (!disposed && canRun.value) {
        if (needsRun) queuePump();
        else scheduleNext();
      }
    }
  }

  function startSession(): void { sessionActive.value = true; }
  function stopSession(): void {
    sessionActive.value = false;
    invalidate(false);
  }
  function resetSession(): void {
    sessionActive.value = false;
    invalidate(true);
  }
  function refresh(): void {
    if (!canRun.value || disposed) return;
    invalidate(false);
    queuePump();
  }
  function togglePause(): void { isPaused.value = !isPaused.value; }
  function setLineLimit(raw: number): void {
    lineLimit.value = clampLineLimit(raw);
    lines.value = boundedLines(lines.value, lineLimit.value);
    writePreference(KEYS.limit, String(lineLimit.value));
    logVersion.value += 1;
  }
  function toggleReverse(): void {
    isReverse.value = !isReverse.value;
    writePreference(KEYS.reverse, isReverse.value ? '1' : '0');
  }
  function toggleAutoScroll(): void {
    autoScroll.value = !autoScroll.value;
    writePreference(KEYS.autoScroll, autoScroll.value ? '1' : '0');
  }
  function acknowledgeNewLines(): void { newLineCount.value = 0; }
  function pollOnce(): void { if (canRun.value) queuePump(); }

  watch(scopeSnapshot, (next, previous) => {
    if (next.every((value, index) => value === previous[index])) return;
    invalidate(true);
    if (canRun.value) queuePump();
  }, { flush: 'sync' });
  // Invalidate on entering AND leaving visibility/pause/switch boundaries.
  watch(canRun, (enabled) => {
    invalidate(false);
    if (enabled) queuePump();
  }, { flush: 'sync' });
  // Clearing on switch begin also covers a failed switch back to the same profile.
  watch(() => switchGuard.switching, (switching) => {
    if (switching) invalidate(true);
    else if (canRun.value) queuePump();
  }, { flush: 'sync' });
  onScopeDispose(() => { disposed = true; invalidate(true); });

  return {
    lines, pendingFragment, offset, logPath, fileSize, isPaused, isLoading, error,
    lineLimit, isReverse, autoScroll, filterText, newLineCount, logVersion,
    displayedLines, totalBuffered, matchedCount, isPolling, requestBlocked, sourceLabel,
    startSession, stopSession, resetSession, refresh, togglePause, setLineLimit,
    toggleReverse, toggleAutoScroll, acknowledgeNewLines, pollOnce,
  };
});

```

## 附录 D — 请求隔离器测试起始文件

目标：`src/tests/unit/port/RequestEpoch.test.ts`。完整照抄；它只验证纯隔离器，不能替代 T04 的 Store 竞态矩阵。

```ts
import { describe, expect, it } from 'vitest';
import { RequestEpoch } from '../../../features/logcenter/requestEpoch';

function begin(gate: RequestEpoch) {
  const ticket = gate.begin();
  if (!ticket) throw new Error('expected a ticket');
  return ticket;
}

describe('RequestEpoch', () => {
  it('allows only one request in the same epoch', () => {
    const gate = new RequestEpoch();
    begin(gate);
    expect(gate.begin()).toBeNull();
  });
  it('invalidates results without pretending the request is cancelled', () => {
    const gate = new RequestEpoch();
    const old = begin(gate);
    gate.invalidate();
    expect(gate.isCurrent(old)).toBe(false);
    expect(gate.inFlight).toBe(1);
  });
  it('caps all unfinished invokes at two across epochs', () => {
    const gate = new RequestEpoch();
    const a = begin(gate);
    gate.invalidate();
    begin(gate);
    gate.invalidate();
    expect(gate.begin()).toBeNull();
    gate.finish(a);
    expect(gate.begin()).not.toBeNull();
    expect(gate.inFlight).toBe(2);
  });
  it('obsolete finally does not clear the newer loading owner', () => {
    const gate = new RequestEpoch();
    const a = begin(gate);
    gate.invalidate();
    const b = begin(gate);
    gate.finish(a);
    expect(gate.isCurrent(b)).toBe(true);
    expect(gate.currentInFlight).toBe(1);
    gate.finish(a);
    expect(gate.inFlight).toBe(1);
    gate.finish(b);
    expect(gate.inFlight).toBe(0);
  });
});
```

## 附录 E — 交付前的文件与来源检查

在仓库根目录运行。阶段/恢复检查保持 `FINAL = False`；T07 最终交付检查改为 `FINAL = True`。只读，不修正任何错误；发现越界或已存在的原样文件被改就退出。

```python
from pathlib import Path
import subprocess

FINAL = False  # T07 最终交付检查必须改为 True
BASE = "bae00da0e3669de9592c838f1b7ff4592591c78d"
ALLOWED = {
    "src/components/ui/RefreshButton.vue",
    "src/core/utils/nameHue.ts",
    "src/core/utils/mention.ts",
    "src/features/logcenter/logText.ts",
    "src-tauri/src/vcp_modules/infra/http_clients.rs",
    "src-tauri/src/vcp_modules/infra/admin_api.rs",
    "src-tauri/src/vcp_modules/logcenter/mod.rs",
    "src-tauri/src/vcp_modules/logcenter/log_service.rs",
    "src/features/logcenter/requestEpoch.ts",
    "src/features/logcenter/logCenterStore.ts",
    "src/features/logcenter/LogCenterView.vue",
    "src/tests/unit/port/DirectAssets.test.ts",
    "src/tests/unit/port/RefreshButton.test.ts",
    "src/tests/unit/port/LowRiskUi.test.ts",
    "src/tests/unit/port/RequestEpoch.test.ts",
    "src/tests/unit/port/LogCenterStore.test.ts",
    "src/tests/unit/port/LogCenterIntegration.test.ts",
    "docs/UPSTREAM_PORT_PHASE1.md",
    "src/main.ts",
    "src/features/rag/RagObserver.vue",
    "src/features/chat/StagedAttachmentPreview.vue",
    "src/features/agent/GroupMembersSection.vue",
    "src-tauri/src/vcp_modules/infra/mod.rs",
    "src-tauri/src/vcp_modules/mod.rs",
    "src-tauri/src/lib.rs",
    "src/core/stores/overlay.ts",
    "src/components/FeatureOverlays.vue",
    "src/components/layout/RightSidebar.vue",
}
EXACT = {
    "src/components/ui/RefreshButton.vue": "17a2ff2753f5681c774105ae6e9ab5c989ec4254",
    "src/core/utils/nameHue.ts": "dbb972edfb2551bacb3af104c43c021b58e95a84",
    "src/core/utils/mention.ts": "d6fcf0e44ed68a4c658eb713dac928b8409b41d2",
    "src/features/logcenter/logText.ts": "8d05a478b09cd5cfe43076a3696b9279865a0941",
    "src-tauri/src/vcp_modules/infra/http_clients.rs": "352651f2cf9b82976a385c81446dda82a3192638",
    "src-tauri/src/vcp_modules/infra/admin_api.rs": "5a36015ec40153752fbdf7988580319ac8e55c67",
}
root = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
def paths_from(args):
    data = subprocess.check_output(["git", *args], cwd=root)
    return {p.decode() for p in data.split(b"\0") if p}
changed = paths_from(["diff", "--name-only", "-z", BASE])
changed |= paths_from(["ls-files", "--others", "--exclude-standard", "-z"])
extra = sorted(changed - ALLOWED)
if extra:
    raise SystemExit("STOP: files outside allowlist: " + ", ".join(extra))
for relative, expected in EXACT.items():
    path = root / relative
    if not path.exists() and not FINAL:
        continue
    if not path.is_file() or path.is_symlink():
        raise SystemExit(f"STOP: missing/invalid exact-copy file: {relative}")
    actual = subprocess.check_output(["git", "hash-object", "--stdin"], input=path.read_bytes(), cwd=root).decode().strip()
    if actual != expected:
        raise SystemExit(f"STOP: exact-copy file changed: {relative}")
for relative in ALLOWED:
    path = root / relative
    if FINAL and not path.exists():
        raise SystemExit(f"STOP: expected deliverable absent: {relative}")
subprocess.check_call(["git", "diff", "--check", BASE], cwd=root)
print("PASS: final presence + scope + hashes" if FINAL else "PASS: partial scope + existing raw-copy hashes")
```

### 编制阶段实际检查记录

本文件编制时已经做过：

- 通过 GitHub 读取两边 main，确认仍分别为本文两个固定 SHA。
- 核对日志页面、共享 HTTP 模块、Settings 读取签名、Overlay 结构、右栏入口、BottomSheet 能力与 invoke 门禁。
- 对附录 B、C 做 TypeScript 语法转换检查；对附录 B 单独执行 11 条 Node 断言，覆盖同代际排他、旧代际失效、总在途上限、旧 finally 不释放新请求。

尚未做过：项目级 Vue 类型检查、Pinia/Vitest Store 集成测试、Rust 编译/测试、前端生产构建、APK 构建、真机或真实服务器互通。上面的有限检查不能代替 T00–T08 的执行和验收。
