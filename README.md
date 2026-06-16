# Baijimu Codex Connector

Baijimu Codex Connector exposes a local `codex app-server` session to Baijimu Local through a local HTTP service.

It is a connector package for `bridge-agent`, not a replacement for Codex. Codex authentication, model access, approvals, sandboxing, and workspace permissions remain owned by the local Codex installation. Baijimu Local controls which remote workspace may call the exposed connector methods.

## Requirements

- Node.js 18 or newer.
- Codex CLI with `codex app-server` available on `PATH`.
- Baijimu Local / `bridge-agent` with connector installation support.

## Install

From a checkout:

```bash
npm install -g .
bridge-agent connector install /path/to/baijimu-connector-codex --replace
bridge-agent connector start com.baijimu.connector.codex
```

Or install the package from a Git remote first:

```bash
npm install -g git+https://gitee.com/zxflimit_admin/baijimu-connector-codex.git
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
- `codexSession.startThread`
- `codexSession.resumeThread`
- `codexSession.startTurn`
- `codexSession.steerTurn`
- `codexSession.interruptTurn`
- `codexSession.recentEvents`
- `codexSession.request`

`request` is an advanced raw JSON-RPC forwarder and should be treated as high risk in remote authorization policies.

## Development

```bash
npm test
```

The tests use a fake app-server process and do not require Codex credentials.
