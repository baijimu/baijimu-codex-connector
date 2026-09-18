# Codex 桌面连接器

连接用户正在运行的 Codex 桌面进程，通过桌面 IPC 发现任务所有者、发送和引导轮次、中断以及响应待处理请求；历史列表从 Codex 原生历史索引只读分页获取。关闭窗口不一定退出桌面进程；真正退出后，Connector 报告 `DESKTOP_IPC_UNAVAILABLE`，不会启动第二个 app-server。

## 运行与边界

使用独立签名原生包，由 Bridge Agent 托管进程。管理接口和业务接口需要宿主注入的 Connector token；业务调用还需要可信工作区 Header。HTTP 仅绑定 loopback。端口默认值来自 `connector.json`，可通过 `--port` 或 `CODEX_CONNECTOR_PORT` 配置。

macOS/Linux 默认连接用户 `.codex/ipc/ipc.sock`；Windows 使用 `\\.\pipe\codex-ipc`。部署方可以通过 `CODEX_DESKTOP_IPC_PATH` 指定实际桌面 IPC 端点。Unix 端点必须是当前用户拥有的 socket。此协议是桌面内部协议，目前实现 initialize v0、owner discovery v1、stream v11 及清单中列出的 follower 请求；不承诺任意桌面版本均兼容。

任务列表和项目目录从桌面 `state_5.sqlite` 只读索引读取，返回 `metadataOnly`，不代表该任务已在桌面打开。部署方可用 `CODEX_DESKTOP_STATE_DB` 指定对应版本的索引。已持久化任务内容从原生历史索引读取；实时运行态和所有操作由 IPC 所有者提供，未找到所有者时需先在桌面打开任务。Connector 不修改桌面数据库、rollout、配置或登录态。

## 从 2.2.0 迁移

- `startThread`、`request`、`listApps` 和文件式 `setThreadReadState` 已移除。独立 app-server 功能使用另一个应用 `digital-employee-connector`。
- `resumeThread` 表示连接桌面所有者并获取快照，不再调用独立 app-server 的 `thread/resume`。
- `readThread` 返回带 `revision` 和 `ownerClientId` 的桌面状态；旧版 `listThreadTurns` 展开 canonical history；新版分页契约见下文迁移说明。
- `steerTurn` 使用桌面当前轮次语义，不接受 app-server 的 `turnId` 条件。`interruptTurn` 要求 `turnId`，通过桌面 `expectedTurnId` 校验。
- 移除 CLI 自动安装、daemon/proxy、transport 切换及 JavaScript app-server 实现。CLI 安装不属于桌面 Connector。
- 桌面事件统一为 `codexDesktopEvent`，保留原始 IPC 消息和修订号。`recentEvents` 返回连接的 `streamId`、序号和有限事件窗口。流重置、序号缺口或修订缺口后重新调用 `readThread`；事件是实时通知，不是持久消息队列。

`IPC_OPERATION_OUTCOME_UNKNOWN` 表示写入后响应丢失，任务可能已经执行。调用方必须读取桌面状态核对，禁止自动重发。连接恢复仅用于新请求，不回放旧写请求，不切换 app-server。

## 验证与发布

`cargo fmt --check`、`cargo test --locked`、`npm test`、`python3 -m unittest discover -s test -p 'test_*.py'`。协议测试覆盖分片帧、所有者路由、快照、断线后写入结果未知和不自动重发。本机只读实测覆盖已有桌面任务和完整历史读取。

唯一发布入口为本仓库 `.github/workflows/release.yml`，从主线精确提交生成三平台签名制品、公开 OSS 内容寻址归档和来源环境冻结版本。中心提交后的 `PENDING_REVIEW` 仍需独立审核。

## 4.0.1 契约修正

`startTurn.cwd` 会传递给桌面所有者。审批同时核对待处理请求的轮次与所有者；不匹配时拒绝发送。项目列表仅提供只读索引的分页、路径搜索、目录存在性和归档筛选；在 4.0.1 中，旧 app-server 的项目扫描选项与 `listThreadTurns.itemsView` 不属于桌面 IPC 契约，显式传入返回 `UNSUPPORTED_PARAMETER`。该版本的任务轮次保留桌面的完整 canonical 数据。


## 新版分页契约（待发布，不兼容 4.x 的轮次返回格式）

分页设计参考 [Happy 消息同步](https://github.com/slopus/happy/blob/3fd0be9e2afb19cce67fed40af379db5e73b7d27/packages/happy-app/sources/sync/sync.ts) 的最新页优先、稳定游标和消息 ID 合并方式；本实现是独立 Rust 桌面适配，不包含 Happy 的 App Server 启动器或后台全历史预取，也未复制其源码。

`listThreadTurns` 不再调用 `thread-follower-load-complete-history`。本地 `state_5.sqlite` 解析任务身份和历史模式，`thread_history_1.sqlite` 提供原生持久化轮次及条目。连接始终只读，不建立第二份消息库、不修改 Codex 状态、不启动独立运行时。部署方可用 `CODEX_DESKTOP_HISTORY_DB` 指定配套的原生历史库路径。必须使用同一 CODEX_HOME/设备的元数据和历史库；路径仅由本机环境配置，远程请求不能指定文件。

当前支持 Codex 原生 `history_mode=paginated` 及已验证的 `thread_turns` / `thread_items` 结构。旧 `legacy` 任务返回 `DESKTOP_HISTORY_MODE_UNSUPPORTED`；未建立投影返回 `DESKTOP_HISTORY_NOT_READY`，不会返回伪造的空历史或自动退回完整加载。旧模式应先在支持分页的桌面版本中打开，原生迁移由 Codex 负责，Connector 不承诺打开一定完成迁移。数据库缺失或结构不匹配返回 `DESKTOP_HISTORY_UNAVAILABLE`。桌面进程退出后，业务入口仍拒绝调用。

### 调用顺序

以下均为 Connector 方法的 `arguments`；工作区设备调用外层保持不变。

1. 读取最新一轮：`listThreadTurns({threadId, limit: 1, sortDirection: "desc", itemsView: "summary"})`。
2. 往上加载：沿返回的 `nextCursor` 再调用同方法，保持任务和排序不变；`null` 表示当前分页快照没有更早内容。
3. 展开轮次：`listThreadItems({threadId, turnId, limit: 20, sortDirection: "asc"})`，继续使用它自己的 `nextCursor`。
4. 展开某条工具输出：`readThreadItem({threadId, turnId, itemId})`，沿游标拼接所有 `content`，最后 `JSON.parse` 得到原生完整条目。
5. 查询/订阅当前桌面状态：`resumeThread({threadId, excludeTurns: true})` 或 `readThread({threadId, includeTurns: false})`。只接收当前所有者快照，不请求完整历史。`pendingRequests` 和审批前检查也使用此路径。

`readThread` / `resumeThread` 未明确排除轮次时仍是显式完整桌面快照接口，不用于首屏历史。IPC 当前状态快照可能包含桌面已经加载的历史；本机过滤只保证这些历史不进入上述元数据响应，不保证 IPC 原始帧大小。

### 返回结构

方法结果仍在 `result` 中。轮次列表：

```json
{
  "data": [{
    "turnId": "…", "status": "completed", "errorPreview": null,
    "startedAt": 0, "completedAt": 1, "durationMs": 1000,
    "itemCount": 42, "itemsView": "summary",
    "items": [{
      "id": "…", "type": "agentMessage", "version": 123,
      "preview": "回答内容预览", "contentBytes": 1024,
      "status": null, "detailAvailable": true, "previewOnly": true
    }]
  }],
  "nextCursor": "不透明游标或null",
  "sortDirection": "desc", "itemsView": "summary",
  "source": "desktop-readonly-history",
  "persistence": {
    "projectedBytes": 123, "observedRolloutBytes": 123,
    "matchesObservedRollout": true, "liveStateSource": "desktop-ipc"
  }
}
```

`startedAt` / `completedAt` 为原生 Unix 秒，`durationMs` 为毫秒。摘要包含首条用户消息与最终助手消息；运行中的轮次尚无最终指针时取最近持久化的助手消息。每条 `preview` 最多 2000 个 Unicode 字符，完整内容始终通过详情获取，不能把预览当作完整回答。`notLoaded` 只返回轮次元数据、条目数和空 `items`。

`listThreadItems` 的 `data` 是同一结构的条目摘要数组，不自动读取下一页。列表每页最多 100 个对象，并有 256 KiB 数据预算；实际返回量可能小于 `limit`，应以 `nextCursor` 判断是否结束。

`readThreadItem` 返回 `threadId`、`turnId`、`itemId`、`version`、`encoding: "json-text"`、`offset`、`totalCharacters`、`content`、`nextCursor`。每段最多 8192 个 Unicode 字符；offset 不是 UTF-8 字节位置，也不是 JavaScript UTF-16 下标。调用方只传回游标，不自行计算 offset；最后一段前不要解析局部 JSON。原文不会被丢弃或截断。

### 增量、一致性和迁移

- 4.x 数字偏移游标不能复用。轮次默认从旧版前 50 轮改为最新 1 轮；轮次的桌面内部字段替换为上述持久化摘要，旧调用方必须适配。`itemsView: "full"` 不属于新列表契约，改用条目分页和内容分段读取。
- 稳定身份是 `(threadId, turnId, itemId)`，不能只用 itemId。相同条目以 `version` 合并，避免重复追加工具调用和助手消息。IPC revision 和持久化条目 version 属于不同域，不能互相比较。
- 游标绑定查询类型、任务、轮次和方向，并固定本次分页的最高序号。新增轮次/条目不会移动旧页；返回最新位置时重新发无 cursor 请求获取新的分页窗口。
- 滚动期间保持旧窗口。`HISTORY_CURSOR_STALE` 表示锚点删除、回退或分段条目已更新，应丢弃旧游标和未拼完的详情，重新获取该窗口/条目，不能拼接两个版本。`INVALID_HISTORY_CURSOR` 表示游标损坏或跨查询复用。
- `persistence` 仅描述读取时观测到的原生持久化进度。`matchesObservedRollout=false` 表示当次观测不一致（可能仍在落盘或投影）；null 表示日志不可观测。即使为 true，也不证明所有实时输出均已落盘。实时执行和待审批状态以 IPC 所有者为准。
- 原始 `codexDesktopEvent` 契约未变化，可能包含大快照。此版本限制的是查询响应；调用方不要通过轮询完整快照更新聊天记录。后续事件传输优化需独立契约，不把原始事件悄悄裁剪。

验证：常规完整测试之外，可显式设置 `CODEX_HISTORY_TEST_THREAD`，运行 `cargo test --locked native_ -- --ignored --nocapture` 做本机只读验证，只打印大小、耗时和投影位置，不打印用户消息。
