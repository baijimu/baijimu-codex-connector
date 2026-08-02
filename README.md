# Baijimu Codex Local App

Baijimu Codex is an independent Rust local application that installs and configures Codex on the current computer, then manages local Codex sessions through one loopback service. The installation flow always uses the workspace currently authorized in Baijimu Local; it does not ask for a device, workspace, or Baijimu project.

It is installed and supervised by `bridge-agent`. Bridge Agent passes only the current workspace ID and waits for setup completion. Credential issuance, exact workspace validation, the official Codex installer, configuration, smoke tests, and process/window verification run inside this application. Bridge Agent never receives the LLM key.

## Requirements

- Baijimu Local / `bridge-agent` 0.2.21 or newer with the `connector.setup.v1` host capability.
- A Baijimu workspace already authorized in the client.

The official market package ships a Rust/native `baijimu-connector-codex`
binary under `bin/<platform>-<arch>/`. The legacy Node.js implementation is
kept for reference and compatibility, but the platform-managed entrypoint is
the native binary.

The package includes `ui/`, a static interface loaded inside the local-app detail panel. It provides setup status and retry, Codex directory/session browsing, newest-first session ordering, new-session creation, and turn execution/interruption. Account state is read-only and reflects the client's current authorized workspace.

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

The connector listens on `127.0.0.1:18110` by default. During market installation Bridge Agent starts it and invokes the declared setup lifecycle. The connector downloads the official script from `docs.baijimu.com`, creates a workspace-scoped LLM credential, passes it through a private temporary file, and removes the file after setup. Before the official installer runs it snapshots the user's default Codex authentication and configuration, and restores both files before activating the isolated workspace profile. It starts `codex app-server --listen stdio://` lazily on the first Codex request.

Bridge Agent assigns a private application data directory through `BAIJIMU_CONNECTOR_DATA_DIR`. The application stores its `management-token`, credential metadata, and one private `CODEX_HOME` per Baijimu environment/user/workspace there. The user's personal ChatGPT login remains in the default `CODEX_HOME`. Switching authentication stops the previous app-server and lazily starts a new one with the selected profile, which also isolates local session history. Existing workspace credentials are validated and reused; a replacement is issued only when the stored credential is no longer valid. Pre-1.2 metadata is migrated on first use. Every `/management/v1/*` request requires the management token. These management routes are local-only and are never registered as relay capabilities.

## CLI

```bash
baijimu-connector-codex start
baijimu-connector-codex start --daemon
baijimu-connector-codex status
baijimu-connector-codex stop
baijimu-connector-codex credential-state
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

The application exposes authenticated setup and status operations for Bridge Agent:

- `GET /management/v1/setup/state`
- `POST /management/v1/setup/retry`
- `GET /management/v1/credential-state`
- `POST /management/v1/auth/switch` with `{ "mode": "chatgpt" }` or `{ "mode": "baijimu", "workspaceId": 123 }`
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
