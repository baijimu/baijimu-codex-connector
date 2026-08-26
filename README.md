# 百积木 Codex 远程连接器

存量市场 appId `codex-connector` 对应面向百积木 Relay 的独立 Codex CLI 远程连接器。它只负责：

- 按百积木权威制品目录安装和定期同步官方 Codex CLI，并通过宿主注入的当前用户 `PATH` 验证版本、`codex` 与 `app-server` 能力；
- 接收 Relay 鉴权后传入的可信 `workspaceId`；
- 使用设备用户的系统默认 `~/.codex` 启动唯一的 Connector app-server；
- 提供 `session/thread/turn/event` 和原始 app-server 请求接口；
- 将平台工作区身份仅作为调用授权上下文，不参与 Codex 状态或进程隔离。

它不安装或启动 ChatGPT/Codex 桌面应用，不创建 Codex 工作区档案，也不签发、切换或覆盖 `~/.codex/auth.json`。桌面安装和系统默认 Codex 认证由 appId `codex`（Codex 桌面管理器）负责；appId `codex-completion`（Codex 模型接口服务）继续独立提供 OpenAI 兼容模型接口，不并入本应用。

## 调用上下文

Relay 从客户端令牌取得平台工作区，发送 `LocalAppInvokeRequest.workspace_id`；Bridge Agent 0.6.0 及以上把该值写入本机 HTTP 请求头 `x-baijimu-workspace-id`。Connector 不接受调用参数中的工作区覆盖。该字段只证明本次调用已经过平台授权，不选择 `CODEX_HOME`、账号、任务空间或 app-server。

所有获准调用都进入同一个、固定绑定系统默认 `~/.codex` 的 app-server。当前 Codex 账号、配置、会话、历史、技能和任务状态完全由该系统默认目录决定。Connector 私有目录只保存自身管理令牌、安装状态、日志和已读状态，不保存平台工作区到 Codex 运行时的映射。

## 本地运行

```bash
cargo run -- start
cargo run -- status
cargo run -- stop
```

默认监听 `127.0.0.1:18111`。Bridge Agent 会注入：

- `BAIJIMU_LOCAL_APP_DATA_DIR`：应用私有状态目录；
- `CODEX_CONNECTOR_BAIJIMU_BINARY`：平台管理的 Baijimu CLI 绝对路径；
- `PATH`：当前桌面用户的规范命令搜索路径；连接器的检查和运行均直接执行同一个 `codex` 命令，不再自行枚举安装目录或探测登录 Shell。

连接器启动时刷新权威 CLI 制品目录，目录校验结果的进程内有效期为 6 小时，过期后的首次调用会再次刷新。目录不可用时使用本机已校验缓存，并以支持分页任务历史的最低协议版本作为最终下限。检测到版本落后时，连接器会等待当前调用结束、停止旧 app-server、安装并复验新 CLI；同步期间调用返回结构化的 `CODEX_CLI_INITIALIZING` 状态。

远程能力、方法、事件、超时和输入 Schema 以 [connector.json](./connector.json) 为准。

## 验证

```bash
cargo test
npm test
```

本仓库是 `codex-connector` 客户端本地应用的唯一发布单元；源码主线为 `baijimu/baijimu-codex-connector/main`，标签、制品、签名和市场版本必须保持同一版本号。
