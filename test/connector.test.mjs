import assert from "node:assert/strict";
import { once } from "node:events";
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { delimiter, dirname, join, resolve } from "node:path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const root = resolve(__dirname, "..");
const cli = join(root, "bin", "baijimu-connector-codex.js");
const fakeCodex = join(__dirname, "fake-codex-app-server.mjs");
const originalHome = process.env.HOME;
const fakeCodexHome = mkdtempSync(join(tmpdir(), "codex-test-home-"));
const fakeCodexBin = join(fakeCodexHome, ".local", "bin");
mkdirSync(fakeCodexBin, { recursive: true });
symlinkSync(process.execPath, join(fakeCodexBin, "codex"));
if (originalHome) {
  process.env.CARGO_HOME ||= join(originalHome, ".cargo");
  process.env.RUSTUP_HOME ||= join(originalHome, ".rustup");
}
process.env.HOME = fakeCodexHome;
process.env.PATH = `${fakeCodexBin}${delimiter}${process.env.PATH || ""}`;
process.on("exit", () => rmSync(fakeCodexHome, { recursive: true, force: true }));

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
    headers: {
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });
  const payload = await response.json();
  assert.equal(response.status, 200, JSON.stringify(payload));
  return payload;
}

async function waitForHealth(port) {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/healthz`);
      if (response.ok) {
        return;
      }
    } catch {
      // Keep polling until the server is ready.
    }
    await delay(50);
  }
  throw new Error("connector did not become healthy");
}

test("forwards thread and turn calls to Codex app-server", async () => {
  const port = await freePort();
  const proc = spawn(process.execPath, [
    cli,
    "start",
    "--port",
    String(port),
    "--codex-args",
    JSON.stringify([fakeCodex]),
  ], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN: "1",
    },
  });

  try {
    await waitForHealth(port);

    const statusBefore = await postJson(port, "/invoke/status");
    assert.equal(statusBefore.data.appServer.running, false);

    const thread = await postJson(port, "/invoke/startThread", {
      model: "gpt-test",
    });
    assert.equal(thread.data.result.thread.id, "thr_test");
    assert.equal(thread.data.status, undefined);

    const statusAfterStart = await postJson(port, "/invoke/status");
    assert.equal(statusAfterStart.data.appServer.running, true);

    const turn = await postJson(port, "/invoke/startTurn", {
      threadId: "thr_test",
      input: "Say hello",
    });
    assert.equal(turn.data.result.turn.id, "turn_test");

    const events = await postJson(port, "/invoke/recentEvents", {
      afterSequence: 0,
      limit: 20,
    });
    assert.ok(events.data.events.some((event) => event.method === "thread/started"));
    assert.ok(events.data.events.some((event) => event.method === "item/agentMessage/delta"));
  } finally {
    if (proc.exitCode === null && proc.signalCode === null) {
      try {
        await fetch(`http://127.0.0.1:${port}/__shutdown`, { method: "POST" });
      } catch {
        proc.kill("SIGTERM");
      }
      const exited = once(proc, "exit");
      await Promise.race([
        exited,
        delay(1000).then(() => {
          proc.kill("SIGKILL");
        }),
      ]);
      if (proc.exitCode === null && proc.signalCode === null) {
        await exited;
      }
    }
    if (process.platform !== "win32") {
      try {
        execFileSync("pkill", ["-f", fakeCodex]);
      } catch {
        // The fake process may already be gone.
      }
    }
  }
});

test("forwards thread and app list APIs to Codex app-server", async () => {
  const port = await freePort();
  const proc = spawn(process.execPath, [
    cli,
    "start",
    "--port",
    String(port),
    "--codex-args",
    JSON.stringify([fakeCodex]),
  ], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN: "1",
    },
  });

  try {
    await waitForHealth(port);

    const threads = await postJson(port, "/invoke/listThreads", { limit: 5 });
    assert.equal(threads.data.result.data[0].id, "thr_listed");
    assert.equal(threads.data.result.data[0].cwd, "/tmp/listed");
    assert.equal(threads.data.result.data[0].requestParams.sortKey, "updated_at");
    assert.equal(threads.data.result.data[0].requestParams.sortDirection, "desc");
    assert.equal(threads.data.status, undefined);

    const explicitlySortedThreads = await postJson(port, "/invoke/listThreads", {
      limit: 5,
      sortKey: "created_at",
      sortDirection: "asc",
    });
    assert.equal(explicitlySortedThreads.data.result.data[0].requestParams.sortKey, "created_at");
    assert.equal(explicitlySortedThreads.data.result.data[0].requestParams.sortDirection, "asc");

    const search = await postJson(port, "/invoke/searchThreads", { searchTerm: "Search" });
    assert.equal(search.data.result.data[0].thread.id, "thr_search");

    const thread = await postJson(port, "/invoke/readThread", {
      threadId: "thr_read",
      includeTurns: true,
    });
    assert.equal(thread.data.result.thread.id, "thr_read");
    assert.equal(thread.data.result.thread.turns[0].id, "turn_read");

    const turns = await postJson(port, "/invoke/listThreadTurns", {
      threadId: "thr_read",
      limit: 8,
      sortDirection: "desc",
      itemsView: "full",
    });
    assert.equal(turns.data.result.data[0].id, "turn_recent");
    assert.equal(turns.data.result.data[0].items[0].text, "limit=8;direction=desc;items=full");
    assert.equal(turns.data.result.nextCursor, "older_cursor");
    assert.equal(turns.data.status, undefined);

    const emptyTurns = await postJson(port, "/invoke/listThreadTurns", {
      threadId: "",
      limit: 8,
    });
    assert.deepEqual(emptyTurns.data.result.data, []);
    assert.equal(emptyTurns.data.result.nextCursor, null);

    const apps = await postJson(port, "/invoke/listApps", { limit: 10 });
    assert.equal(apps.data.result.data[0].id, "app_test");
    assert.equal(apps.data.result.data[0].isAccessible, true);
    assert.equal(apps.data.status, undefined);
  } finally {
    if (proc.exitCode === null && proc.signalCode === null) {
      try {
        await fetch(`http://127.0.0.1:${port}/__shutdown`, { method: "POST" });
      } catch {
        proc.kill("SIGTERM");
      }
      const exited = once(proc, "exit");
      await Promise.race([
        exited,
        delay(1000).then(() => {
          proc.kill("SIGKILL");
        }),
      ]);
      if (proc.exitCode === null && proc.signalCode === null) {
        await exited;
      }
    }
  }
});

test("flattens wrapped Codex thread list items", async () => {
  const port = await freePort();
  const proc = spawn(process.execPath, [
    cli,
    "start",
    "--port",
    String(port),
    "--codex-args",
    JSON.stringify([fakeCodex]),
  ], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN: "1",
      CODEX_FAKE_WRAP_THREAD_LIST: "1",
    },
  });

  try {
    await waitForHealth(port);

    const threads = await postJson(port, "/invoke/listThreads", { limit: 5 });
    const thread = threads.data.result.data[0];
    assert.equal(thread.id, "thr_wrapped");
    assert.equal(thread.name, "Wrapped Thread");
    assert.equal(thread.cwd, "/tmp/wrapped");
    assert.equal(thread.thread.id, "thr_wrapped");
    assert.equal(thread.wrapperMeta, "kept");
    assert.equal(thread.requestParams.sortKey, "updated_at");

    const sessions = await postJson(port, "/invoke/listSessions", { limit: 5 });
    assert.equal(sessions.data.result.data[0].id, "thr_wrapped");
    assert.equal(sessions.data.result.data[0].thread.id, "thr_wrapped");
  } finally {
    if (proc.exitCode === null && proc.signalCode === null) {
      try {
        await fetch(`http://127.0.0.1:${port}/__shutdown`, { method: "POST" });
      } catch {
        proc.kill("SIGTERM");
      }
      const exited = once(proc, "exit");
      await Promise.race([
        exited,
        delay(1000).then(() => {
          proc.kill("SIGKILL");
        }),
      ]);
      if (proc.exitCode === null && proc.signalCode === null) {
        await exited;
      }
    }
  }
});

test("lists Codex projects separately from sessions", async () => {
  const port = await freePort();
  const codexHome = await mkdtemp(join(tmpdir(), "codex-home-"));
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

  const proc = spawn(process.execPath, [
    cli,
    "start",
    "--port",
    String(port),
    "--codex-args",
    JSON.stringify([fakeCodex]),
  ], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      CODEX_HOME: codexHome,
      CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN: "1",
    },
  });

  try {
    await waitForHealth(port);

    const response = await postJson(port, "/invoke/listProjects", { limit: 20 });
    const projects = response.data.result.projects;
    const byPath = new Map(projects.map((project) => [project.path, project]));

    assert.equal(response.data.result.total, 4);
    assert.equal(response.data.status, undefined);
    assert.equal(byPath.get(savedProject).pinned, true);
    assert.ok(byPath.get(savedProject).sources.includes("saved"));
    assert.equal(byPath.get(activeProject).active, true);
    assert.ok(byPath.get(activeProject).sources.includes("active"));
    assert.equal(byPath.get(trustedProject).trustLevel, "trusted");
    assert.ok(byPath.get("/tmp/listed").sources.includes("threads"));
    assert.equal(byPath.get("/tmp/listed").sessionCount, 1);

    const sessions = await postJson(port, "/invoke/listSessions", { limit: 5 });
    assert.equal(sessions.data.result.data[0].id, "thr_listed");
    assert.equal(sessions.data.result.data[0].requestParams.sortKey, "updated_at");
    assert.equal(sessions.data.result.data[0].requestParams.sortDirection, "desc");
  } finally {
    if (proc.exitCode === null && proc.signalCode === null) {
      try {
        await fetch(`http://127.0.0.1:${port}/__shutdown`, { method: "POST" });
      } catch {
        proc.kill("SIGTERM");
      }
      const exited = once(proc, "exit");
      await Promise.race([
        exited,
        delay(1000).then(() => {
          proc.kill("SIGKILL");
        }),
      ]);
      if (proc.exitCode === null && proc.signalCode === null) {
        await exited;
      }
    }
    await rm(codexHome, { recursive: true, force: true });
  }
});

test("resolves current Codex project IDs to their real roots", async () => {
  const port = await freePort();
  const codexHome = await mkdtemp(join(tmpdir(), "codex-home-current-projects-"));
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

  const proc = spawn(process.execPath, [
    cli,
    "start",
    "--port",
    String(port),
    "--codex-args",
    JSON.stringify([fakeCodex]),
  ], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      CODEX_HOME: codexHome,
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
    if (proc.exitCode === null && proc.signalCode === null) {
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
    await rm(codexHome, { recursive: true, force: true });
  }
});

test("falls back to readThread when paginated turn listing is unavailable", async () => {
  const port = await freePort();
  const proc = spawn(process.execPath, [
    cli,
    "start",
    "--port",
    String(port),
    "--codex-args",
    JSON.stringify([fakeCodex]),
  ], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN: "1",
      CODEX_FAKE_DISABLE_TURNS_LIST: "1",
    },
  });

  try {
    await waitForHealth(port);

    const turns = await postJson(port, "/invoke/listThreadTurns", {
      threadId: "thr_read",
      limit: 8,
      itemsView: "full",
    });
    assert.equal(turns.data.result.data[0].id, "turn_read");
    assert.equal(turns.data.result.fallback, "thread/read");
    assert.equal(turns.data.status, undefined);

    const events = await postJson(port, "/invoke/recentEvents", {
      afterSequence: 0,
      limit: 20,
    });
    assert.ok(events.data.events.some((event) => event.method === "connector/threadTurnsListFallback"));
  } finally {
    if (proc.exitCode === null && proc.signalCode === null) {
      try {
        await fetch(`http://127.0.0.1:${port}/__shutdown`, { method: "POST" });
      } catch {
        proc.kill("SIGTERM");
      }
      const exited = once(proc, "exit");
      await Promise.race([
        exited,
        delay(1000).then(() => {
          proc.kill("SIGKILL");
        }),
      ]);
      if (proc.exitCode === null && proc.signalCode === null) {
        await exited;
      }
    }
  }
});

test("serializes first app-server initialization across concurrent requests", async () => {
  const port = await freePort();
  const proc = spawn(process.execPath, [
    cli,
    "start",
    "--port",
    String(port),
    "--codex-args",
    JSON.stringify([fakeCodex]),
  ], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN: "1",
    },
  });

  try {
    await waitForHealth(port);
    const [threads, apps] = await Promise.all([
      postJson(port, "/invoke/listThreads", { limit: 1 }),
      postJson(port, "/invoke/listApps", { limit: 1 }),
    ]);
    assert.equal(threads.data.result.data[0].id, "thr_listed");
    assert.equal(apps.data.result.data[0].id, "app_test");
  } finally {
    if (proc.exitCode === null && proc.signalCode === null) {
      try {
        await fetch(`http://127.0.0.1:${port}/__shutdown`, { method: "POST" });
      } catch {
        proc.kill("SIGTERM");
      }
      const exited = once(proc, "exit");
      await Promise.race([
        exited,
        delay(1000).then(() => {
          proc.kill("SIGKILL");
        }),
      ]);
      if (proc.exitCode === null && proc.signalCode === null) {
        await exited;
      }
    }
  }
});

test("daemon mode starts with file descriptor backed logs", async () => {
  const port = await freePort();
  const home = await mkdtemp(join(tmpdir(), "baijimu-connector-codex-"));

  try {
    const output = execFileSync(process.execPath, [
      cli,
      "start",
      "--daemon",
      "--port",
      String(port),
      "--codex-args",
      JSON.stringify([fakeCodex]),
    ], {
      cwd: root,
      encoding: "utf8",
      env: {
        ...process.env,
        CODEX_CONNECTOR_HOME: home,
        CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN: "1",
      },
    });
    const started = JSON.parse(output);
    assert.equal(started.ok, true);
    assert.equal(started.url, `http://127.0.0.1:${port}`);

    await waitForHealth(port);
    const pid = Number((await readFile(join(home, "connector.pid"), "utf8")).trim());
    assert.equal(pid, started.pid);

    await fetch(`http://127.0.0.1:${port}/__shutdown`, { method: "POST" });
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});
