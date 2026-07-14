# Baijimu Codex Connector

Baijimu Codex Connector exposes a local `codex app-server` session to Baijimu Local through a local HTTP service.

It is a connector package for `bridge-agent`, not a replacement for Codex. Codex authentication, model access, approvals, sandboxing, and workspace permissions remain owned by the local Codex installation. Baijimu Local controls which remote workspace may call the exposed connector methods.

## Requirements

- Codex CLI with `codex app-server` available on `PATH`.
- Baijimu Local / `bridge-agent` with connector installation support.

The official market package ships a Rust/native `baijimu-connector-codex`
binary under `bin/<platform>-<arch>/`. The legacy Node.js implementation is
kept for reference and compatibility, but the platform-managed entrypoint is
the native binary.

## Install

From a checkout:

```bash
cargo build --release
bridge-agent connector install /path/to/baijimu-connector-codex --replace
bridge-agent connector start com.baijimu.connector.codex
```

Or install the tagged package from a Git remote first:

```bash
git clone https://gitee.com/zxflimit_admin/baijimu-connector-codex.git
bridge-agent connector install /path/to/baijimu-connector-codex --replace
```

The connector listens on `127.0.0.1:18110` by default. It starts `codex app-server --listen stdio://` lazily on the first Codex request.

## CLI

```bash
baijimu-connector-codex start
baijimu-connector-codex start --daemon
baijimu-connector-codex status
baijimu-connector-codex stop
```

Configuration can be provided with flags or environment variables:

```bash
CODEX_CONNECTOR_PORT=18110
CODEX_CONNECTOR_CODEX_BINARY=codex
CODEX_CONNECTOR_CODEX_ARGS='["app-server","--listen","stdio://"]'
```

## Bridge Service

The connector registers one service:

- `codexSession.status`
- `codexSession.listThreads`
- `codexSession.searchThreads`
- `codexSession.readThread`
- `codexSession.listApps`
- `codexSession.startThread`
- `codexSession.resumeThread`
- `codexSession.startTurn`
- `codexSession.steerTurn`
- `codexSession.interruptTurn`
- `codexSession.recentEvents`
- `codexSession.request`

`request` is an advanced raw JSON-RPC forwarder and should be treated as high risk in remote authorization policies.

Thread list responses include the Codex `cwd`, source, git metadata, title, preview, and pagination cursors so callers can choose the right workspace before starting or resuming work.

## Development

```bash
cargo test
npm run test:rust
```

The integration tests use a fake app-server process and do not require Codex
credentials.
