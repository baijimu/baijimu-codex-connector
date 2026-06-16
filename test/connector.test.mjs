import assert from "node:assert/strict";
import { once } from "node:events";
import { execFileSync, spawn } from "node:child_process";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const root = resolve(__dirname, "..");
const cli = join(root, "bin", "baijimu-connector-codex.js");
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
    "--codex-binary",
    process.execPath,
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
    assert.equal(thread.data.status.appServer.running, true);

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
