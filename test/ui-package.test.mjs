import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import {
  buildSwitchPayload,
  credentialStatusMeta,
  normalizeCredentialState,
  normalizeCodexSessions,
  codexTurnMessages,
  preferredWorkspaceId,
} from "../ui/state.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

test("connector manifest declares the packaged embedded UI", async () => {
  const manifest = JSON.parse(await readFile(join(root, "connector.json"), "utf8"));
  assert.equal(manifest.schemaVersion, "2.0");
  assert.equal(manifest.version, "1.0.0");
  assert.equal(manifest.transport.type, "http");
  assert.ok(manifest.methods.some((method) => method.name === "status"));
  assert.ok(manifest.events.some((event) => event.name === "codexNotification"));
  assert.equal(manifest.services, undefined);
  assert.equal(manifest.serviceRegistrationFiles, undefined);
  assert.deepEqual(manifest.ui, {
    type: "embedded",
    entry: "ui/index.html",
    title: "Codex 远程开发",
    defaultView: true,
  });
  assert.deepEqual(Object.keys(manifest.management.operations).sort(), [
    "checkoutPlatformProject",
    "credentialState",
    "interruptCodexTurn",
    "listCodexProjects",
    "listCodexSessions",
    "listCodexTurns",
    "listWorkspaceProjects",
    "readCodexSession",
    "recentCodexEvents",
    "startCodexSession",
    "startCodexTurn",
    "switchCredential",
  ]);
  const html = await readFile(join(root, manifest.ui.entry), "utf8");
  assert.match(html, /src="\.\/app\.js"/);
  assert.match(html, /href="\.\/styles\.css"/);
  assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)[^>]*>/i);
  await readFile(join(root, "ui", "app.js"), "utf8");
  await readFile(join(root, "ui", "state.mjs"), "utf8");
  await readFile(join(root, "ui", "styles.css"), "utf8");
});

test("UI keeps Codex sessions newest-first and extracts conversation messages", () => {
  const sessions = normalizeCodexSessions([
    { id: "old", name: "Old", updated_at: 100 },
    { id: "new", name: "New", updated_at: 200 },
  ]);
  assert.deepEqual(sessions.map((session) => session.id), ["new", "old"]);
  assert.deepEqual(codexTurnMessages([{
    id: "turn-1",
    items: [
      { id: "user-1", type: "user_message", text: "实现功能" },
      { id: "agent-1", type: "agent_message", content: [{ text: "已经完成" }] },
    ],
  }]), [
    { id: "user-1", role: "user", text: "实现功能" },
    { id: "agent-1", role: "assistant", text: "已经完成" },
  ]);
});

test("UI state normalizes management responses and keeps the active workspace", () => {
  const state = normalizeCredentialState({
    codexConfigured: true,
    credentialStatus: "verified",
    activeProfile: {
      workspaceId: 642,
      workspaceName: "研发",
      projectId: 7405,
      projectName: "Codex",
      model: "gpt-5.6-sol",
      activatedAtEpochSeconds: 123,
    },
    profiles: [],
    workspaces: [
      { workspaceId: 100, name: "其他" },
      { workspaceId: 642, name: "研发" },
    ],
  });
  assert.equal(state.codexConfigured, true);
  assert.equal(preferredWorkspaceId(state), 642);
  assert.deepEqual(credentialStatusMeta(state.credentialStatus), { label: "已验证", tone: "success" });
});

test("UI builds only complete, explicitly scoped credential switch requests", () => {
  assert.deepEqual(buildSwitchPayload({
    workspaceId: "642",
    workspaceName: "研发",
    projectId: "7405",
    projectName: "Codex",
    model: "gpt-5.6-sol",
  }), {
    workspaceId: 642,
    workspaceName: "研发",
    projectId: 7405,
    projectName: "Codex",
    model: "gpt-5.6-sol",
  });
  assert.throws(() => buildSwitchPayload({ workspaceId: 642, projectId: 0, model: "gpt-5.6-sol" }), /项目 ID/);
  assert.throws(() => buildSwitchPayload({ workspaceId: 642, projectId: 7405, model: " " }), /模型不能为空/);
});
