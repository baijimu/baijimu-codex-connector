import assert from "node:assert/strict";
import { once } from "node:events";
import { execFileSync, spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const root = resolve(__dirname, "..");
const cli = join(root, "target", "debug", "baijimu-connector-codex");
const fakeCodex = join(__dirname, "fake-codex-app-server.mjs");

async function freePort() {
  const { createServer } = await import("node:net");
  const server = createServer();
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const port = server.address().port;
  server.close();
  await once(server, "close");
  return port;
}

async function postJson(port, path, body = {}) {
  const response = await fetch(`http://127.0.0.1:${port}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const payload = await response.json();
  assert.equal(response.status, 200, JSON.stringify(payload));
  return payload;
}

async function postManagementJson(port, token, path, body = {}) {
  const response = await fetch(`http://127.0.0.1:${port}${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${token}`,
    },
    body: JSON.stringify(body),
  });
  const payload = await response.json();
  assert.equal(response.status, 200, JSON.stringify(payload));
  assert.equal(payload.ok, true, JSON.stringify(payload));
  return payload.data;
}

async function waitForHealth(port) {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/healthz`);
      if (response.ok) return;
    } catch {
      // Keep polling until the server is ready.
    }
    await delay(50);
  }
  throw new Error("connector did not become healthy");
}

async function stopConnector(proc, port) {
  if (proc.exitCode !== null || proc.signalCode !== null) return;
  try {
    await fetch(`http://127.0.0.1:${port}/__shutdown`, { method: "POST" });
  } catch {
    proc.kill("SIGTERM");
  }
  const exited = once(proc, "exit");
  await Promise.race([
    exited,
    delay(1000).then(() => proc.kill("SIGKILL")),
  ]);
}

test("rust connector forwards Codex app-server calls", async () => {
  execFileSync("cargo", ["build"], { cwd: root, stdio: "inherit" });
  const port = await freePort();
  const connectorHome = await mkdtemp(join(tmpdir(), "codex-app-data-"));
  const proc = spawn(cli, [
    "start",
    "--port",
    String(port),
    "--codex-binary",
    process.execPath,
    "--codex-args",
    JSON.stringify([fakeCodex]),
  ], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      BAIJIMU_CONNECTOR_DATA_DIR: connectorHome,
      CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN: "1",
    },
  });

  try {
    await waitForHealth(port);

    const unauthorized = await fetch(`http://127.0.0.1:${port}/management/v1/credential-state`);
    assert.equal(unauthorized.status, 401);
    const managementToken = (await readFile(join(connectorHome, "management-token"), "utf8")).trim();
    assert.ok(managementToken.length >= 32);
    const authorizedUnknown = await fetch(`http://127.0.0.1:${port}/management/v1/unknown`, {
      headers: { authorization: `Bearer ${managementToken}` },
    });
    assert.equal(authorizedUnknown.status, 404);

    const managedSessions = await postManagementJson(
      port,
      managementToken,
      "/management/v1/codex/sessions",
      { limit: 5, sortKey: "updated_at", sortDirection: "desc" },
    );
    assert.equal(managedSessions.result.data[0].id, "thr_listed");
    assert.equal(managedSessions.result.data[0].requestParams.sortKey, "updated_at");

    const managedThread = await postManagementJson(
      port,
      managementToken,
      "/management/v1/codex/sessions/start",
      { model: "gpt-test", cwd: "/tmp/project" },
    );
    assert.equal(managedThread.result.thread.id, "thr_test");

    const managedTurn = await postManagementJson(
      port,
      managementToken,
      "/management/v1/codex/turns/start",
      { threadId: "thr_test", input: "Say hello" },
    );
    assert.equal(managedTurn.result.turn.id, "turn_test");

    const thread = await postJson(port, "/invoke/startThread", { model: "gpt-test" });
    assert.equal(thread.data.result.thread.id, "thr_test");
    assert.equal(thread.data.status, undefined);

    const threads = await postJson(port, "/invoke/listThreads", { limit: 5 });
    assert.equal(threads.data.result.data[0].id, "thr_listed");
    assert.equal(threads.data.result.data[0].requestParams.sortKey, "updated_at");

    const turns = await postJson(port, "/invoke/listThreadTurns", {
      threadId: "thr_read",
      limit: 8,
      sortDirection: "desc",
      itemsView: "full",
    });
    assert.equal(turns.data.result.data[0].id, "turn_recent");

    const turn = await postJson(port, "/invoke/startTurn", {
      threadId: "thr_test",
      input: "Say hello",
    });
    assert.equal(turn.data.result.turn.id, "turn_test");

    const events = await postJson(port, "/invoke/recentEvents", {
      afterSequence: 0,
      limit: 20,
    });
    assert.ok(events.data.events.some((event) => event.method === "item/agentMessage/delta"));
  } finally {
    await stopConnector(proc, port);
    await rm(connectorHome, { recursive: true, force: true });
  }
});

test("rust connector lists Codex projects and falls back for turns", async () => {
  execFileSync("cargo", ["build"], { cwd: root, stdio: "inherit" });
  const port = await freePort();
  const codexHome = await mkdtemp(join(tmpdir(), "codex-home-"));
  const connectorHome = await mkdtemp(join(tmpdir(), "codex-app-data-"));
  const savedProject = join(codexHome, "saved-project");
  const activeProject = join(codexHome, "active-project");
  const trustedProject = join(codexHome, "trusted-project");
  await mkdir(savedProject, { recursive: true });
  await mkdir(activeProject, { recursive: true });
  await mkdir(trustedProject, { recursive: true });
  await writeFile(join(codexHome, ".codex-global-state.json"), JSON.stringify({
    "project-order": [savedProject],
    "electron-saved-workspace-roots": [savedProject, activeProject],
    "active-workspace-roots": [activeProject],
    "pinned-project-ids": [savedProject],
  }));
  await writeFile(join(codexHome, "config.toml"), `[projects."${trustedProject}"]\ntrust_level = "trusted"\n`);

  const proc = spawn(cli, [
    "start",
    "--port",
    String(port),
    "--codex-binary",
    process.execPath,
    "--codex-args",
    JSON.stringify([fakeCodex]),
  ], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      CODEX_HOME: codexHome,
      BAIJIMU_CONNECTOR_DATA_DIR: connectorHome,
      CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN: "1",
      CODEX_FAKE_DISABLE_TURNS_LIST: "1",
    },
  });

  try {
    await waitForHealth(port);

    const response = await postJson(port, "/invoke/listProjects", { limit: 20 });
    const byPath = new Map(response.data.result.projects.map((project) => [project.path, project]));
    assert.equal(response.data.result.total, 4);
    assert.equal(byPath.get(savedProject).pinned, true);
    assert.equal(byPath.get(activeProject).active, true);
    assert.equal(byPath.get(trustedProject).trustLevel, "trusted");
    assert.ok(byPath.get("/tmp/listed").sources.includes("threads"));

    const turns = await postJson(port, "/invoke/listThreadTurns", {
      threadId: "thr_read",
      limit: 8,
      itemsView: "full",
    });
    assert.equal(turns.data.result.data[0].id, "turn_read");
    assert.equal(turns.data.result.fallback, "thread/read");

    const events = await postJson(port, "/invoke/recentEvents", {
      afterSequence: 0,
      limit: 20,
    });
    assert.ok(events.data.events.some((event) => event.method === "connector/threadTurnsListFallback"));
  } finally {
    await stopConnector(proc, port);
    await rm(codexHome, { recursive: true, force: true });
    await rm(connectorHome, { recursive: true, force: true });
  }
});

test("rust connector resolves current Codex project IDs to their real roots", async () => {
  execFileSync("cargo", ["build"], { cwd: root, stdio: "inherit" });
  const port = await freePort();
  const codexHome = await mkdtemp(join(tmpdir(), "codex-home-current-projects-"));
  const connectorHome = await mkdtemp(join(tmpdir(), "codex-app-data-"));
  const projectRoot = "/tmp/listed";
  await writeFile(join(codexHome, ".codex-global-state.json"), JSON.stringify({
    "project-order": ["local-listed", "local-unresolved"],
    "pinned-project-ids": ["local-listed"],
    "local-projects": {
      "local-listed": {
        id: "local-listed",
        name: "Listed Project",
        rootPaths: [projectRoot],
      },
    },
  }));

  const proc = spawn(cli, [
    "start",
    "--port",
    String(port),
    "--codex-binary",
    process.execPath,
    "--codex-args",
    JSON.stringify([fakeCodex]),
  ], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      CODEX_HOME: codexHome,
      BAIJIMU_CONNECTOR_DATA_DIR: connectorHome,
      CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN: "1",
    },
  });

  try {
    await waitForHealth(port);
    const response = await postJson(port, "/invoke/listProjects", { limit: 20 });
    assert.equal(response.data.result.total, 1);
    const [project] = response.data.result.projects;
    assert.equal(project.id, projectRoot);
    assert.equal(project.path, projectRoot);
    assert.equal(project.projectId, "local-listed");
    assert.equal(project.projectName, "Listed Project");
    assert.equal(project.title, "Listed Project");
    assert.deepEqual(project.rootPaths, [projectRoot]);
    assert.equal(project.pinned, true);
    assert.equal(project.sessionCount, 1);
    assert.deepEqual(project.sources, ["saved", "pinned", "threads"]);
    assert.ok(!project.path.includes("local-unresolved"));
  } finally {
    await stopConnector(proc, port);
    await rm(codexHome, { recursive: true, force: true });
    await rm(connectorHome, { recursive: true, force: true });
  }
});

test("rust connector daemon mode writes pid file", async () => {
  execFileSync("cargo", ["build"], { cwd: root, stdio: "inherit" });
  const port = await freePort();
  const home = await mkdtemp(join(tmpdir(), "baijimu-connector-codex-"));

  try {
    const output = execFileSync(cli, [
      "start",
      "--daemon",
      "--port",
      String(port),
      "--codex-binary",
      process.execPath,
      "--codex-args",
      JSON.stringify([fakeCodex]),
    ], {
      cwd: root,
      encoding: "utf8",
      env: {
        ...process.env,
        BAIJIMU_CONNECTOR_DATA_DIR: home,
        CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN: "1",
      },
    });
    const started = JSON.parse(output);
    assert.equal(started.ok, true);
    assert.equal(started.url, `http://127.0.0.1:${port}`);

    await waitForHealth(port);
    await delay(750);
    await waitForHealth(port);
    const pid = Number((await readFile(join(home, "connector.pid"), "utf8")).trim());
    assert.equal(pid, started.pid);

    await fetch(`http://127.0.0.1:${port}/__shutdown`, { method: "POST" });
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});
