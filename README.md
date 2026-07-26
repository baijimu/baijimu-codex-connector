# Baijimu Codex Local App

Baijimu Codex is an independent Rust local application that manages local Codex sessions, Baijimu workspaces, and Codex LLM credentials through one loopback service. The application package also owns its embedded account and workspace interface; Bridge Agent only hosts that interface and proxies the management operations declared by this connector.

It is installed and supervised by `bridge-agent`, but credential issuance, ownership validation, atomic Codex configuration updates, and Codex process restart are executed inside this application. Bridge Agent never receives the LLM key. Baijimu Local still controls which remote workspace may call the exposed session methods.

## Requirements

- Codex CLI with `codex app-server` available on `PATH`.
- Baijimu Local / `bridge-agent` with connector installation support.

The official market package ships a Rust/native `baijimu-connector-codex`
binary under `bin/<platform>-<arch>/`. The legacy Node.js implementation is
kept for reference and compatibility, but the platform-managed entrypoint is
the native binary.

The package includes `ui/`, a static interface loaded inside the local-app detail panel. It provides Codex project/session browsing, newest-first session ordering, new-session creation, turn execution/interruption, and account/workspace switching. Every UI action goes through an explicitly declared `window.baijimuLocalApp` management operation protected by the connector token; the page cannot access Tauri commands, relay methods, local files, or arbitrary HTTP endpoints.

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

Bridge Agent assigns a private application data directory through `BAIJIMU_CONNECTOR_DATA_DIR`. The application stores its `management-token` and credential profile metadata there with private permissions, and migrates pre-0.4 profile metadata from the old shared location on first use. Every `/management/v1/*` request requires the management token. These management routes are local-only and are never registered as relay capabilities.

## CLI

```bash
baijimu-connector-codex start
baijimu-connector-codex start --daemon
baijimu-connector-codex status
baijimu-connector-codex stop
baijimu-connector-codex credential-state
baijimu-connector-codex list-workspace-projects --workspace-id 642
baijimu-connector-codex switch-credential --workspace-id 642 --workspace-name "工作区" --project-id 7405 --project-name "项目"
baijimu-connector-codex checkout-project --workspace-id 642 --project-id 7405
```

Configuration can be provided with flags or environment variables:

```bash
CODEX_CONNECTOR_PORT=18110
CODEX_CONNECTOR_CODEX_BINARY=codex
CODEX_CONNECTOR_BAIJIMU_BINARY=baijimu
CODEX_CONNECTOR_PROJECTS_DIR=/absolute/path/to/Baijimu/Projects
CODEX_CONNECTOR_CODEX_ARGS='["app-server","--listen","stdio://"]'
```

## Local App Capabilities

The `schemaVersion: "2.0"` manifest declares these methods directly on
`connectorId=com.baijimu.connector.codex`; installation does not create a runtime service or
businessId:

- `status`
- `listThreads`
- `searchThreads`
- `readThread`
- `listApps`
- `startThread`
- `resumeThread`
- `startTurn`
- `steerTurn`
- `interruptTurn`
- `recentEvents`
- `request`

`request` is an advanced raw JSON-RPC forwarder and should be treated as high risk in remote authorization policies.

## Local management API

The application exposes three authenticated operations for the Bridge Agent application panel:

- `GET /management/v1/credential-state`
- `POST /management/v1/workspace-projects`
- `POST /management/v1/switch-credential`
- `POST /management/v1/projects/checkout`

The local management token is not a Baijimu workspace token or an LLM credential. It only authenticates the loopback call between Bridge Agent and this application.

Thread list responses include the Codex `cwd`, source, git metadata, title, preview, and pagination cursors so callers can choose the right workspace before starting or resuming work.

The project checkout operation delegates to the managed `baijimu project checkout`
command. It creates or validates a stable local checkout under
`CODEX_CONNECTOR_PROJECTS_DIR`, uses the platform Git credential helper, and
returns the canonical directory and current `codex/<userId>/...` branch for a
new Codex session. Existing directories are reused only after their Baijimu
workspace/project metadata, origin URL, and Codex branch namespace all match.

## Development

```bash
cargo test
npm run test:rust
```

The integration tests use a fake app-server process and do not require Codex
credentials.
