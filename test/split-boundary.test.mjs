import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFile(join(root, path), "utf8");

test("Connector manifest exposes only CLI and remote app-server responsibilities", async () => {
  const manifest = JSON.parse(await read("connector.json"));
  assert.equal(manifest.id, "com.baijimu.connector.codex-connector");
  assert.equal(manifest.name, "Codex 外部连接器");
  assert.equal(manifest.version, "1.0.0");
  assert.equal(manifest.source.repo, "momoplan/baijimu-codex-connector");
  assert.equal(manifest.runtime.healthCheck.url, "http://127.0.0.1:18111/healthz");
  assert.equal(manifest.hostRequirements.minimumVersion, "0.3.0");
  assert.equal(manifest.ui, undefined);
  assert.equal(manifest.management.operations.credentialState, undefined);
  assert.equal(manifest.management.operations.launchCodex, undefined);
  assert.ok(manifest.methods.some((method) => method.name === "startThread"));
  assert.ok(manifest.events.some((event) => event.name === "codexTurnCompleted"));
});

test("Connector requires trusted workspace context and owns isolated profiles", async () => {
  const [main, profile, readme] = await Promise.all([
    read("src/main.rs"),
    read("src/workspace_profile.rs"),
    read("README.md"),
  ]);
  assert.match(main, /x-baijimu-workspace-id/);
  assert.match(main, /WORKSPACE_CONTEXT_REQUIRED/);
  assert.match(profile, /workspace-profiles/);
  assert.match(profile, /environment: String/);
  assert.match(readme, /codex-completion`（Codex 补全服务）继续独立/);
});

test("Connector setup executes CLI installation from its own artifact catalog", async () => {
  const [source, artifactSource] = await Promise.all([
    read("src/setup/macos.rs"),
    read("installers/upstream-artifact-source.json"),
  ]);
  const execute = source.slice(source.indexOf("fn execute"), source.indexOf("fn ensure_desktop_app"));
  assert.match(execute, /ensure_codex_cli/);
  assert.doesNotMatch(execute, /ensure_desktop_app/);
  assert.doesNotMatch(execute, /launch_desktop/);
  assert.match(artifactSource, /codex-artifacts\/v4/);
});
