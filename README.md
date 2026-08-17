# Baijimu Codex Connector

`com.baijimu.connector.codex` 是面向百积木 Relay 的 Codex CLI Connector。它只负责：

- 安装、发现并验证官方 Codex CLI 与 `app-server` 能力；
- 接收 Relay 鉴权后传入的可信 `workspaceId`；
- 为每个 `(environment, workspaceId)` 建立独立的 Connector 私有 `CODEX_HOME`；
- 提供 `session/thread/turn/event` 和原始 app-server 请求接口；
- 按工作区隔离 app-server 进程、凭证、任务状态和已读状态。

它不安装或启动 ChatGPT/Codex 桌面应用，也不管理桌面当前工作区。桌面安装与工作区切换由独立的 `com.baijimu.connector.codex-desktop` 本地应用负责。`com.baijimu.connector.codex-completion` 继续独立提供直接使用底层 Codex 能力的 OpenAI 兼容补全接口。

## 调用上下文

Relay 从客户端令牌取得工作区，发送 `LocalAppInvokeRequest.workspace_id`；Bridge Agent 0.3.0 及以上把该值写入本机 HTTP 请求头 `x-baijimu-workspace-id`。Connector 不接受调用参数中的工作区覆盖，也不读取桌面管理器的当前选择。

Connector 首次处理某工作区调用时，通过平台管理的 Baijimu CLI 校验设备授权、读取工作区并签发专用 LLM credential。档案位于 Connector 的 `BAIJIMU_CONNECTOR_DATA_DIR/workspace-profiles` 下，路径同时包含环境和工作区身份；不同工作区不会共享 `auth.json`、`config.toml` 或 app-server。

## 本地运行

```bash
cargo run -- start
cargo run -- status
cargo run -- stop
```

默认监听 `127.0.0.1:18110`。Bridge Agent 会注入：

- `BAIJIMU_CONNECTOR_DATA_DIR`：应用私有状态目录；
- `CODEX_CONNECTOR_BAIJIMU_BINARY`：平台管理的 Baijimu CLI 绝对路径。

远程能力、方法、事件、超时和输入 Schema 以 [connector.json](./connector.json) 为准。

## 验证

```bash
cargo test
npm test
```

发布由独立发布任务执行；本仓库的标签、制品、签名和市场版本必须保持同一版本号。
