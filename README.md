# Codex 桌面连接器 4.0.0

连接用户正在运行的 Codex 桌面进程，通过桌面 IPC 发现任务所有者、读取完整任务历史、发送和引导轮次、中断以及响应待处理请求。关闭窗口不一定退出桌面进程；真正退出后，Connector 报告 `DESKTOP_IPC_UNAVAILABLE`，不会启动第二个 app-server。

## 运行与边界

使用独立签名原生包，由 Bridge Agent 托管进程。管理接口和业务接口需要宿主注入的 Connector token；业务调用还需要可信工作区 Header。HTTP 仅绑定 loopback。端口默认值来自 `connector.json`，可通过 `--port` 或 `CODEX_CONNECTOR_PORT` 配置。

macOS/Linux 默认连接用户 `.codex/ipc/ipc.sock`；Windows 使用 `\\.\pipe\codex-ipc`。部署方可以通过 `CODEX_DESKTOP_IPC_PATH` 指定实际桌面 IPC 端点。Unix 端点必须是当前用户拥有的 socket。此协议是桌面内部协议，目前实现 initialize v0、owner discovery v1、stream v11 及清单中列出的 follower 请求；不承诺任意桌面版本均兼容。

任务列表和项目目录从桌面 `state_5.sqlite` 只读索引读取，返回 `metadataOnly`，不代表该任务已在桌面打开。部署方可用 `CODEX_DESKTOP_STATE_DB` 指定对应版本的索引。任务内容、运行态和所有操作由 IPC 所有者提供；未找到所有者时需先在桌面打开任务。Connector 不修改桌面数据库、rollout、配置或登录态。

## 从 2.2.0 迁移

- `startThread`、`request`、`listApps` 和文件式 `setThreadReadState` 已移除。独立 app-server 功能使用另一个应用 `digital-employee-connector`。
- `resumeThread` 表示连接桌面所有者并获取快照，不再调用独立 app-server 的 `thread/resume`。
- `readThread` 返回带 `revision` 和 `ownerClientId` 的桌面状态；`listThreadTurns` 展开 canonical history，保留桌面轮次字段。
- `steerTurn` 使用桌面当前轮次语义，不接受 app-server 的 `turnId` 条件。`interruptTurn` 要求 `turnId`，通过桌面 `expectedTurnId` 校验。
- 移除 CLI 自动安装、daemon/proxy、transport 切换及 JavaScript app-server 实现。CLI 安装不属于桌面 Connector。
- 桌面事件统一为 `codexDesktopEvent`，保留原始 IPC 消息和修订号。`recentEvents` 返回连接的 `streamId`、序号和有限事件窗口。流重置、序号缺口或修订缺口后重新调用 `readThread`；事件是实时通知，不是持久消息队列。

`IPC_OPERATION_OUTCOME_UNKNOWN` 表示写入后响应丢失，任务可能已经执行。调用方必须读取桌面状态核对，禁止自动重发。连接恢复仅用于新请求，不回放旧写请求，不切换 app-server。

## 验证与发布

`cargo fmt --check`、`cargo test --locked`、`npm test`、`python3 -m unittest discover -s test -p 'test_*.py'`。协议测试覆盖分片帧、所有者路由、快照、断线后写入结果未知和不自动重发。本机只读实测覆盖已有桌面任务和完整历史读取。

唯一发布入口为本仓库 `.github/workflows/release.yml`，从主线精确提交生成三平台签名制品、公开 OSS 内容寻址归档和来源环境冻结版本。中心提交后的 `PENDING_REVIEW` 仍需独立审核。
