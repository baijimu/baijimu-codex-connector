import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import {
  connectorStartupRetryable,
  credentialStatusMeta,
  normalizeCredentialState,
  normalizeSetupProgress,
  setupPageMeta,
  setupStatusMeta,
  shouldShowSetupProgress,
} from "../ui/state.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

test("connector manifest declares the packaged embedded UI", async () => {
  const manifest = JSON.parse(await readFile(join(root, "connector.json"), "utf8"));
  const packageManifest = JSON.parse(
    await readFile(join(root, "package.json"), "utf8"),
  );
  assert.equal(manifest.schemaVersion, "2.0");
  assert.equal(manifest.version, packageManifest.version);
  assert.equal(manifest.source.type, "github");
  assert.equal(manifest.source.repo, "momoplan/baijimu-connector-codex");
  assert.equal(manifest.source.revision, `v${manifest.version}`);
  assert.equal(manifest.transport.type, "http");
  assert.ok(manifest.methods.some((method) => method.name === "status"));
  assert.deepEqual(
    manifest.events.map((event) => event.name),
    [
      "codexNotification",
      "codexTurnCompleted",
      "codexThreadClosed",
      "codexThreadArchived",
      "codexThreadDeleted",
    ],
  );
  const turnCompleted = manifest.events.find((event) => event.name === "codexTurnCompleted");
  assert.equal(turnCompleted.enabled, true);
  assert.deepEqual(turnCompleted.payload_schema.properties.status.enum, [
    "completed",
    "interrupted",
    "failed",
  ]);
  assert.equal(turnCompleted.payload_schema.additionalProperties, false);
  assert.equal(manifest.services, undefined);
  assert.equal(manifest.serviceRegistrationFiles, undefined);
  assert.deepEqual(manifest.runtime.args, ["start"]);
  assert.deepEqual(manifest.runtime.stopArgs, ["stop"]);
  assert.equal(manifest.runtime.processOwnership, "host");
  assert.deepEqual(manifest.runtime.healthCheck, {
    type: "http",
    url: "http://127.0.0.1:18110/healthz",
    timeoutSecs: 2,
    expectStatus: 200,
  });
  assert.deepEqual(manifest.ui, {
    type: "embedded",
    entry: "ui/index.html",
    title: "Codex 工作区管理",
    defaultView: true,
  });
  assert.deepEqual(Object.keys(manifest.management.operations).sort(), [
    "checkoutPlatformProject",
    "credentialState",
    "ensureCodexReady",
    "interruptCodexTurn",
    "launchCodex",
    "listCodexProjects",
    "listCodexSessions",
    "listCodexTurns",
    "readCodexSession",
    "recentCodexEvents",
    "restoreExternalCodexHome",
    "setCodexThreadReadState",
    "setupRetry",
    "setupState",
    "startCodexSession",
    "startCodexTurn",
  ]);
  assert.equal(manifest.setup, undefined);
  assert.deepEqual(manifest.hostRequirements, {
    minimumVersion: "0.2.82",
    capabilities: [
      "connector.process.host-managed.v1",
      "connector.managed-tool-dependencies.v1",
    ],
  });
  assert.deepEqual(manifest.managedToolDependencies, [{
    id: "com.baijimu.cli",
    minimumVersion: "0.1.43",
    requiredFor: ["install", "start"],
    executablePathEnv: "CODEX_CONNECTOR_BAIJIMU_BINARY",
  }]);
  assert.equal(manifest.configSchema.properties.codexBinary, undefined);
  assert.equal(manifest.configSchema.properties.baijimuBinary, undefined);
  const html = await readFile(join(root, manifest.ui.entry), "utf8");
  assert.match(html, /src="\.\/app\.js"/);
  assert.match(html, /href="\.\/styles\.css"/);
  assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)[^>]*>/i);
  assert.doesNotMatch(html, /项目 ID|确认设备/);
  assert.match(html, /选择工作区并启动 Codex/);
  assert.match(html, /auth-switch-modal/);
  assert.match(html, /error-retry-button/);
  assert.match(html, /restore-external-home-button/);
  assert.match(html, /全新用户会直接初始化默认 \.codex/);
  assert.match(html, /id="management-active-panel"[^>]*hidden/);
  assert.match(html, /id="management-workspace-panel"[^>]*hidden/);
  assert.match(html, /id="setup-panel"/);
  const app = await readFile(join(root, "ui", "app.js"), "utf8");
  assert.match(app, /启动个人 Codex/);
  assert.match(app, /不会删除任何工作区目录/);
  assert.doesNotMatch(app, /window\.confirm/);
  assert.match(app, /openAuthSwitchModal/);
  assert.match(app, /invokeManagement\("launchCodex"/);
  assert.doesNotMatch(app, /switchAuthProfile|openCodexDesktop/);
  assert.doesNotMatch(app, /请关闭后重新打开/);
  assert.match(app, /invokeManagement\("ensureCodexReady"/);
  assert.match(app, /重新安装并修复/);
  assert.match(app, /showError/);
  assert.match(app, /重试启动/);
  assert.match(app, /重新检查/);
  assert.match(app, /connectorStartupRetryable/);
  assert.match(app, /restoreExternalCodexHome/);
  assert.match(app, /setupPageMeta/);
  assert.match(app, /management-workspace-panel/);
  assert.doesNotMatch(app, /Promise\.all\(\[loadSessions\(\), loadState\(\)\]\)/);
  await readFile(join(root, "ui", "state.mjs"), "utf8");
  await readFile(join(root, "ui", "styles.css"), "utf8");
});

test("UI keeps setup isolated until initialization succeeds", () => {
  assert.deepEqual(setupPageMeta({ status: "pending" }), {
    mode: "setup",
    title: "正在准备安装 Codex",
    description: "正在检查本机环境并准备初始化，请保持此页面打开。",
  });
  assert.equal(setupPageMeta({ status: "running" }).mode, "setup");
  assert.equal(setupPageMeta({ status: "failed" }).mode, "setup");
  assert.equal(setupPageMeta({ status: "interrupted" }).mode, "setup");
  assert.equal(setupPageMeta({ status: "needs_retry" }).mode, "setup");
  assert.deepEqual(setupPageMeta({ status: "succeeded" }), {
    mode: "management",
    title: "Codex 工作区管理",
    description: "选择个人环境或百积木工作区并启动 Codex；所有状态目录彼此隔离。",
  });
});

test("UI retries only the bounded Connector startup response", () => {
  assert.equal(connectorStartupRetryable({ code: "connector_initializing" }), true);
  assert.equal(
    connectorStartupRetryable(new Error("正在初始化 Codex Connector（HTTP 503 Service Unavailable）")),
    true,
  );
  assert.equal(connectorStartupRetryable(new Error("HTTP 503 Service Unavailable")), false);
  assert.equal(connectorStartupRetryable(new Error("credential service unavailable")), false);
  assert.equal(connectorStartupRetryable({ code: "connector_initialization_failed" }), false);
});

test("UI derives Codex initialization progress from installer steps and download bytes", () => {
  const progress = normalizeSetupProgress({
    status: "running",
    installerStatus: {
      currentStep: 3,
      steps: [
        { index: 1, name: "Check App", state: "completed", detail: "Ready" },
        { index: 2, name: "Read manifest", state: "skipped", detail: "Not needed" },
        {
          index: 3,
          name: "Download App",
          state: "running",
          detail: "Downloading",
          downloadedBytes: 50,
          totalBytes: 100,
        },
        { index: 4, name: "Install App", state: "pending" },
      ],
    },
  });
  assert.equal(progress.status, "running");
  assert.equal(progress.currentStep, 3);
  assert.equal(progress.percent, 63);
  assert.equal(progress.steps[2].downloadedBytes, 50);
  assert.equal(progress.steps[2].totalBytes, 100);
  assert.equal(normalizeSetupProgress({ status: "succeeded", installerStatus: { steps: [] } }).percent, 100);
  assert.equal(shouldShowSetupProgress({ status: "running", installerStatus: { steps: [{ state: "running" }] } }), true);
  assert.equal(shouldShowSetupProgress({ status: "succeeded", installerStatus: { steps: [{ state: "completed" }] } }), false);
  assert.equal(shouldShowSetupProgress({ status: "needs_retry", installerStatus: { steps: [{ state: "failed" }] } }), false);
});

test("UI distinguishes retryable setup states from current failures", () => {
  assert.deepEqual(setupStatusMeta({ status: "failed", error: "current" }), {
    status: "failed",
    label: "初始化失败",
    retryable: true,
    showCurrentError: true,
  });
  assert.deepEqual(setupStatusMeta({ status: "interrupted" }), {
    status: "interrupted",
    label: "初始化已中断",
    retryable: true,
    showCurrentError: false,
  });
  assert.deepEqual(setupStatusMeta({ status: "needs_retry", error: "stale" }), {
    status: "needs_retry",
    label: "需要重新验证",
    retryable: true,
    showCurrentError: false,
  });
  assert.equal(setupStatusMeta({ status: "succeeded" }).retryable, false);
});

test("UI state normalizes management responses and keeps the active workspace", () => {
  const state = normalizeCredentialState({
    activeMode: "baijimu",
    codexConfigured: true,
    currentWorkspaceId: 642,
    activeWorkspaceId: 642,
    credentialStatus: "verified",
    activeProfile: {
      workspaceId: 642,
      workspaceName: "研发",
      model: "gpt-5.6-sol",
      activatedAtEpochSeconds: 123,
    },
    profiles: [],
    workspaces: [
      { workspaceId: 100, name: "其他", authorized: false },
      { workspaceId: 642, name: "研发", authorized: true, configured: true, userIds: [25] },
    ],
    chatgpt: { configured: true, authMode: "chatgpt", accountId: "acct-1" },
    originalCodexHome: "/users/test/.codex",
    originalCodexHomeState: { captured: true, value: null, captureSource: "user-environment" },
    activeCodexHome: "/isolated/workspace-642",
    externalCodexHome: "/isolated/workspace-642",
    legacyGlobalCodexHome: {
      restoreRequired: true,
      canRestore: true,
      currentValue: "/isolated/workspace-642",
      restoreValue: null,
      restoredAtEpochSeconds: null,
    },
  });
  assert.equal(state.codexConfigured, true);
  assert.equal(state.currentWorkspaceId, 642);
  assert.equal(state.activeMode, "baijimu");
  assert.equal(state.activeWorkspaceId, 642);
  assert.equal(state.workspaces[1].authorized, true);
  assert.deepEqual(state.workspaces[1].userIds, [25]);
  assert.equal(state.chatgpt.accountId, "acct-1");
  assert.equal(state.originalCodexHome, "/users/test/.codex");
  assert.equal(state.originalCodexHomeState.captured, true);
  assert.equal(state.originalCodexHomeState.wasSet, false);
  assert.equal(state.externalCodexHome, "/isolated/workspace-642");
  assert.equal(state.legacyGlobalCodexHome.restoreRequired, true);
  assert.equal(state.legacyGlobalCodexHome.canRestore, true);
  assert.equal(state.legacyGlobalCodexHome.restoreValue, "");
  assert.equal(state.activeProfile.workspaceId, 642);
  assert.deepEqual(credentialStatusMeta(state.credentialStatus), { label: "已验证", tone: "success" });
});
