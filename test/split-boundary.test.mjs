import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFile(join(root, path), "utf8");

test("Connector manifest exposes only CLI and remote app-server responsibilities", async () => {
  const manifest = JSON.parse(await read("connector.json"));
  assert.equal(manifest.schemaVersion, "3.0.0");
  assert.equal(manifest.appId, "codex-connector");
  assert.equal(manifest.name, "Codex 远程连接器");
  assert.equal(manifest.version, "2.0.5");
  assert.equal(manifest.source.repo, "baijimu/baijimu-codex-connector");
  assert.equal(manifest.source.revision, `v${manifest.version}`);
  assert.equal(manifest.runtime.healthCheck.url, "http://127.0.0.1:18111/healthz");
  assert.equal(manifest.hostRequirements.minimumVersion, "0.6.0");
  assert.equal(manifest.ui, undefined);
  assert.equal(manifest.management.operations.credentialState, undefined);
  assert.equal(manifest.management.operations.launchCodex, undefined);
  assert.ok(manifest.methods.some((method) => method.name === "startThread"));
  assert.ok(!manifest.methods.some((method) => method.name === "listWorkspaceProfiles"));
  assert.ok(manifest.events.some((event) => event.name === "codexTurnCompleted"));
});

test("Connector uses trusted platform authorization with one system Codex home", async () => {
  const [main, runtime, appServer, readme] = await Promise.all([
    read("src/main.rs"),
    read("src/process_runtime.rs"),
    read("src/app_server.rs"),
    read("README.md"),
  ]);
  assert.match(main, /x-baijimu-workspace-id/);
  assert.match(main, /WORKSPACE_CONTEXT_REQUIRED/);
  assert.doesNotMatch(main, /WorkspaceClients|workspace_profile/);
  assert.match(runtime, /home_dir\(\)\.join\("\.codex"\)/);
  assert.match(appServer, /system_codex_home\(\)/);
  assert.doesNotMatch(appServer, /default-profile/);
  assert.doesNotMatch(readme, /workspace-profiles/);
  assert.match(
    readme,
    /codex-completion`（Codex 模型接口服务）继续独立/,
  );
  assert.match(readme, /Bridge Agent 0\.6\.0 及以上/);
});

test("Connector setup executes CLI installation from its own artifact catalog", async () => {
  const [source, catalog, artifactSource] = await Promise.all([
    read("src/setup/macos.rs"),
    read("src/setup/catalog.rs"),
    read("installers/upstream-artifact-source.json"),
  ]);
  const execute = source.slice(source.indexOf("fn execute"), source.indexOf("fn ensure_desktop_app"));
  assert.match(execute, /ensure_codex_cli/);
  assert.doesNotMatch(execute, /ensure_desktop_app/);
  assert.doesNotMatch(execute, /launch_desktop/);
  assert.match(catalog, /CATALOG_REFRESH_INTERVAL/);
  assert.match(catalog, /PAGINATED_THREADS_MINIMUM_VERSION/);
  assert.match(catalog, /verified_cache/);
  assert.match(artifactSource, /codex-artifacts\/v4/);
});

test("Connector uses the host PATH as the only Codex runtime selection contract", async () => {
  const [binary, appServer, windowsInstaller] = await Promise.all([
    read("src/codex_binary.rs"),
    read("src/app_server.rs"),
    read("installers/windows-configure-terminal-and-login.ps1"),
  ]);
  assert.match(binary, /pub const COMMAND: &str = "codex"/);
  assert.match(binary, /Command::new\(command\)/);
  assert.match(appServer, /Command::new\(codex_binary::COMMAND\)/);
  assert.doesNotMatch(binary, /known_codex_candidates|resolve_from_login_environment|Get-Command|WindowsApps|homebrew|snap\/bin/);
  assert.doesNotMatch(appServer, /codex_binary::resolve|codex_binary::Resolution/);
  assert.doesNotMatch(windowsInstaller, /function Get-SystemCodexCli|function Get-ConfiguredCodexCli/);
});

test("CLI installers publish the installed command directory into the user PATH", async () => {
  const [macosInstaller, windowsInstaller] = await Promise.all([
    read("installers/macos-configure-terminal-and-login.sh"),
    read("installers/windows-configure-terminal-and-login.ps1"),
  ]);
  assert.match(macosInstaller, /shell_name="\${SHELL##\*\/}"/);
  assert.match(macosInstaller, /zsh\) profile="\$HOME\/\.zprofile"/);
  assert.doesNotMatch(macosInstaller, /profile="\$HOME\/\.zshrc"/);
  assert.match(windowsInstaller, /SetEnvironmentVariable\(\s*"Path",[\s\S]+?"User"/);
});
