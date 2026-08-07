# Baijimu Codex Local App

Baijimu Codex is an independent Rust local application that initializes and configures Codex on the current computer, then manages local Codex sessions through one loopback service. Initialization uses the workspace currently authorized in Baijimu Local; it does not ask for a device or unrelated Baijimu project.

It is installed and supervised by `bridge-agent`. Bridge Agent finishes Connector installation without waiting for Codex initialization. The user starts initialization inside this application's account view, which shows the official installer's step and download progress. Credential issuance, exact workspace validation, the official Codex installer, configuration, smoke tests, and process/window verification run inside this application. Bridge Agent never receives the LLM key.

## Requirements

- Baijimu Local / `bridge-agent` 0.2.21 or newer with the `connector.setup.v1` host capability.
- A Baijimu workspace already authorized in the client.

The official market package ships a Rust/native `baijimu-connector-codex`
binary under `bin/<platform>-<arch>/`. The legacy Node.js implementation is
kept for reference and compatibility, but the platform-managed entrypoint is
the native binary.

The package includes `ui/`, a static interface loaded inside the local-app detail panel. It provides initialization start/retry, the full installer step list and progress, Codex directory/session browsing, newest-first session ordering, new-session creation, and turn execution/interruption. Account state is read-only and reflects the client's current authorized workspace.

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

The connector listens on `127.0.0.1:18110` by default. Market installation only installs and starts the Connector. Initialization begins from the application's account view, then downloads the official script from `docs.baijimu.com`, creates a workspace-scoped LLM credential, passes it through a private temporary file, and removes the file after setup. The installer receives the isolated workspace directory through `CODEX_HOME`; it never writes the personal `~/.codex` directory or restarts an existing ChatGPT desktop session. If a usable personal ChatGPT login or an explicit profile selection already exists, setup preserves that selection. On a clean machine, setup automatically activates the new workspace profile so the first request works without another user step. It starts `codex app-server --listen stdio://` lazily on the first Codex request.

Codex CLI discovery belongs to this Connector and does not depend on Bridge Agent selecting an executable from its generic desktop-process `PATH`. Automatic mode searches the process `PATH`, official CLI install locations, the Connector-managed content-addressed Windows CLI installation, and finally the user's login environment. It never copies or executes a Codex binary from inside the Windows desktop-app package. `codexBinary` has no default and is an advanced override only; when set, it must be an absolute executable or Windows command-launcher path and automatic discovery is disabled. The `status` response exposes the selected mode, resolved path, source, version, `app-server` capability check, checked paths, and actionable errors.

On Windows, discovery only accepts launchers supported by the native process API (`.exe`, `.com`, `.bat`, or `.cmd`) and follows `PATHEXT`; it never executes the extensionless POSIX shim created beside an npm command. To repair installations created by older Bridge Agent versions, an explicit extensionless `...\codex` path is resolved to a launchable sibling such as `codex.cmd`. If no supported sibling exists, startup fails with `CODEX_BINARY_NOT_FOUND` instead of reaching Windows `ERROR_BAD_EXE_FORMAT` (`os error 193`).

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

`CODEX_CONNECTOR_CODEX_BINARY` is normally left as `codex`; the Connector resolves it independently from the Bridge Agent process environment. Set an absolute path only when intentionally pinning a specific executable. An invalid explicit path fails with `CODEX_BINARY_NOT_FOUND` instead of silently selecting a different installation.

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
