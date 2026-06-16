#!/usr/bin/env node

import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { createWriteStream, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const VERSION = "0.1.0";
const DEFAULT_HOST = "127.0.0.1";
const DEFAULT_PORT = 18110;
const DEFAULT_CODEX_BINARY = "codex";
const DEFAULT_LISTEN = "stdio://";
const DEFAULT_REQUEST_TIMEOUT_MS = 120000;
const MAX_EVENTS = 1000;

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const packageRoot = resolve(__dirname, "..");

class HttpError extends Error {
  constructor(statusCode, message) {
    super(message);
    this.statusCode = statusCode;
  }
}

class CodexAppServerClient {
  constructor(options = {}) {
    this.codexBinary = options.codexBinary || DEFAULT_CODEX_BINARY;
    this.listen = options.listen || DEFAULT_LISTEN;
    this.extraArgs = options.extraArgs || [];
    this.requestTimeoutMs = options.requestTimeoutMs || DEFAULT_REQUEST_TIMEOUT_MS;
    this.clientInfo = options.clientInfo || {
      name: "baijimu_connector_codex",
      title: "Baijimu Codex Connector",
      version: VERSION,
    };
    this.experimentalApi = options.experimentalApi ?? true;
    this.proc = null;
    this.rl = null;
    this.initialized = false;
    this.nextId = 1;
    this.pending = new Map();
    this.events = [];
    this.eventSequence = 0;
    this.startedAt = null;
    this.lastExit = null;
  }

  status() {
    return {
      connector: {
        name: "@baijimu/connector-codex",
        version: VERSION,
        pid: process.pid,
      },
      appServer: {
        running: Boolean(this.proc && this.proc.exitCode === null && !this.proc.killed),
        initialized: this.initialized,
        pid: this.proc?.pid ?? null,
        codexBinary: this.codexBinary,
        listen: this.listen,
        startedAt: this.startedAt,
        lastExit: this.lastExit,
      },
      events: {
        latestSequence: this.eventSequence,
        retained: this.events.length,
      },
    };
  }

  async ensureStarted() {
    if (this.proc && this.proc.exitCode === null && !this.proc.killed && this.initialized) {
      return;
    }

    if (!this.proc || this.proc.exitCode !== null || this.proc.killed) {
      this.startProcess();
    }

    if (!this.initialized) {
      await this.initialize();
    }
  }

  startProcess() {
    const args = this.extraArgs.length > 0
      ? this.extraArgs
      : ["app-server", "--listen", this.listen];
    this.proc = spawn(this.codexBinary, args, {
      cwd: process.cwd(),
      stdio: ["pipe", "pipe", "pipe"],
      env: process.env,
      detached: process.platform !== "win32",
    });
    this.startedAt = new Date().toISOString();
    this.lastExit = null;
    this.initialized = false;

    this.rl = createInterface({ input: this.proc.stdout });
    this.rl.on("line", (line) => this.handleLine(line));
    this.proc.stderr.on("data", (chunk) => {
      this.pushEvent("connector/codexStderr", {
        text: chunk.toString("utf8"),
      });
    });
    this.proc.on("exit", (code, signal) => {
      this.lastExit = { code, signal, at: new Date().toISOString() };
      this.initialized = false;
      this.rejectPending(new Error(`codex app-server exited: code=${code} signal=${signal}`));
    });
    this.proc.on("error", (error) => {
      this.lastExit = { error: error.message, at: new Date().toISOString() };
      this.initialized = false;
      this.rejectPending(error);
    });
  }

  async initialize() {
    const params = {
      clientInfo: this.clientInfo,
      capabilities: {
        experimentalApi: this.experimentalApi,
      },
    };
    const result = await this.request("initialize", params, 30000, { skipEnsureStarted: true });
    this.sendNotification("initialized", {});
    this.initialized = true;
    return result;
  }

  async request(method, params = {}, timeoutMs = this.requestTimeoutMs, options = {}) {
    if (!options.skipEnsureStarted) {
      await this.ensureStarted();
    }
    if (!this.proc || !this.proc.stdin.writable) {
      throw new Error("codex app-server is not writable");
    }

    const id = this.nextId++;
    const message = { method, id, params };
    const result = await new Promise((resolvePromise, rejectPromise) => {
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        rejectPromise(new Error(`codex app-server request timed out: ${method}`));
      }, timeoutMs);

      this.pending.set(id, {
        method,
        resolve: resolvePromise,
        reject: rejectPromise,
        timeout,
      });

      this.proc.stdin.write(`${JSON.stringify(message)}\n`, (error) => {
        if (error) {
          clearTimeout(timeout);
          this.pending.delete(id);
          rejectPromise(error);
        }
      });
    });
    return result;
  }

  sendNotification(method, params = {}) {
    if (!this.proc || !this.proc.stdin.writable) {
      throw new Error("codex app-server is not writable");
    }
    this.proc.stdin.write(`${JSON.stringify({ method, params })}\n`);
  }

  handleLine(line) {
    if (!line.trim()) {
      return;
    }

    let message;
    try {
      message = JSON.parse(line);
    } catch (error) {
      this.pushEvent("connector/parseError", {
        line,
        error: error.message,
      });
      return;
    }

    if (Object.prototype.hasOwnProperty.call(message, "id")) {
      const pending = this.pending.get(message.id);
      if (!pending) {
        this.pushEvent("connector/unmatchedResponse", message);
        return;
      }
      this.pending.delete(message.id);
      clearTimeout(pending.timeout);
      if (message.error) {
        const error = new Error(message.error.message || `codex app-server error for ${pending.method}`);
        error.code = message.error.code;
        error.data = message.error.data;
        pending.reject(error);
      } else {
        pending.resolve(message.result ?? null);
      }
      return;
    }

    this.pushEvent(message.method || "codex/notification", message.params ?? message);
  }

  pushEvent(method, params) {
    const event = {
      sequence: ++this.eventSequence,
      receivedAt: new Date().toISOString(),
      method,
      params,
    };
    this.events.push(event);
    if (this.events.length > MAX_EVENTS) {
      this.events.splice(0, this.events.length - MAX_EVENTS);
    }
  }

  recentEvents({ afterSequence = 0, limit = 100 } = {}) {
    const boundedLimit = Math.max(1, Math.min(Number(limit) || 100, 500));
    return {
      latestSequence: this.eventSequence,
      events: this.events
        .filter((event) => event.sequence > Number(afterSequence || 0))
        .slice(-boundedLimit),
    };
  }

  rejectPending(error) {
    for (const [id, pending] of this.pending.entries()) {
      clearTimeout(pending.timeout);
      pending.reject(error);
      this.pending.delete(id);
    }
  }

  async shutdown() {
    const child = this.proc;
    if (!child || child.exitCode !== null) {
      return;
    }
    child.stdin?.end();
    const exited = new Promise((resolvePromise) => child.once("exit", resolvePromise));
    killChildProcess(child, "SIGTERM");
    await Promise.race([
      exited,
      new Promise((resolvePromise) => setTimeout(resolvePromise, 500)).then(() => {
        if (child.exitCode === null) {
          killChildProcess(child, "SIGKILL");
        }
      }),
    ]);
    if (child.exitCode === null) {
      await Promise.race([
        exited,
        new Promise((resolvePromise) => setTimeout(resolvePromise, 500)),
      ]);
    }
  }
}

function killChildProcess(child, signal) {
  if (process.platform !== "win32" && child.pid) {
    try {
      process.kill(-child.pid, signal);
      return;
    } catch {
      // Fall back to direct child signaling below.
    }
  }
  child.kill(signal);
}

function parseArgs(argv) {
  const [command = "help", ...rest] = argv;
  const options = { command, positional: [] };
  for (let index = 0; index < rest.length; index += 1) {
    const arg = rest[index];
    if (!arg.startsWith("--")) {
      options.positional.push(arg);
      continue;
    }
    const [key, inlineValue] = arg.slice(2).split("=", 2);
    if (["daemon", "help", "version"].includes(key)) {
      options[key] = true;
      continue;
    }
    const value = inlineValue ?? rest[++index];
    if (value === undefined) {
      throw new HttpError(2, `missing value for --${key}`);
    }
    options[toCamelCase(key)] = value;
  }
  return options;
}

function toCamelCase(value) {
  return value.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}

function connectorHome() {
  return process.env.CODEX_CONNECTOR_HOME || join(homedir(), ".baijimu-connector-codex");
}

function pidPath() {
  return join(connectorHome(), "connector.pid");
}

function logPath() {
  return join(connectorHome(), "connector.log");
}

function ensureConnectorHome() {
  mkdirSync(connectorHome(), { recursive: true });
}

function readJsonEnv(name, fallback) {
  const raw = process.env[name];
  if (!raw) {
    return fallback;
  }
  return JSON.parse(raw);
}

function serverOptions(options) {
  return {
    host: options.host || process.env.CODEX_CONNECTOR_HOST || DEFAULT_HOST,
    port: Number(options.port || process.env.CODEX_CONNECTOR_PORT || DEFAULT_PORT),
    codexBinary: options.codexBinary || process.env.CODEX_CONNECTOR_CODEX_BINARY || DEFAULT_CODEX_BINARY,
    listen: options.listen || process.env.CODEX_CONNECTOR_LISTEN || DEFAULT_LISTEN,
    extraArgs: options.codexArgs
      ? JSON.parse(options.codexArgs)
      : readJsonEnv("CODEX_CONNECTOR_CODEX_ARGS", []),
    requestTimeoutMs: Number(options.requestTimeoutMs || process.env.CODEX_CONNECTOR_REQUEST_TIMEOUT_MS || DEFAULT_REQUEST_TIMEOUT_MS),
  };
}

function daemonize(options) {
  ensureConnectorHome();
  const childArgs = [
    __filename,
    "start",
    "--host",
    options.host,
    "--port",
    String(options.port),
    "--codex-binary",
    options.codexBinary,
    "--listen",
    options.listen,
  ];
  if (options.extraArgs?.length) {
    childArgs.push("--codex-args", JSON.stringify(options.extraArgs));
  }

  const log = createWriteStream(logPath(), { flags: "a" });
  const child = spawn(process.execPath, childArgs, {
    cwd: packageRoot,
    detached: true,
    stdio: ["ignore", log, log],
    env: process.env,
  });
  child.unref();
  writeFileSync(pidPath(), `${child.pid}\n`);
  console.log(JSON.stringify({
    ok: true,
    pid: child.pid,
    url: `http://${options.host}:${options.port}`,
    logPath: logPath(),
  }));
}

async function readJsonRequest(request) {
  const chunks = [];
  for await (const chunk of request) {
    chunks.push(chunk);
  }
  if (chunks.length === 0) {
    return {};
  }
  const body = Buffer.concat(chunks).toString("utf8").trim();
  return body ? JSON.parse(body) : {};
}

function writeJson(response, statusCode, payload) {
  response.writeHead(statusCode, {
    "content-type": "application/json; charset=utf-8",
  });
  response.end(`${JSON.stringify(payload)}\n`);
}

function normalizeInput(input) {
  if (typeof input === "string") {
    return [{ type: "text", text: input }];
  }
  if (Array.isArray(input)) {
    return input;
  }
  return input;
}

function mergeParams(body, base = {}) {
  const params = body.params && typeof body.params === "object" ? body.params : {};
  return { ...base, ...params };
}

async function handleInvoke(pathname, body, client) {
  switch (pathname) {
    case "/invoke/status":
      return client.status();
    case "/invoke/startThread": {
      const params = mergeParams(body, {
        ...(body.model ? { model: body.model } : {}),
        ...(body.cwd ? { cwd: body.cwd } : {}),
      });
      const result = await client.request("thread/start", params, body.timeoutMs);
      return { result, status: client.status() };
    }
    case "/invoke/resumeThread": {
      if (!body.threadId) {
        throw new HttpError(400, "threadId is required");
      }
      const params = mergeParams(body, { threadId: body.threadId });
      const result = await client.request("thread/resume", params, body.timeoutMs);
      return { result, status: client.status() };
    }
    case "/invoke/startTurn": {
      if (!body.threadId) {
        throw new HttpError(400, "threadId is required");
      }
      if (body.input === undefined) {
        throw new HttpError(400, "input is required");
      }
      const params = mergeParams(body, {
        threadId: body.threadId,
        input: normalizeInput(body.input),
        ...(body.model ? { model: body.model } : {}),
        ...(body.cwd ? { cwd: body.cwd } : {}),
      });
      const result = await client.request("turn/start", params, body.timeoutMs);
      return { result, recentEvents: client.recentEvents({ limit: 50 }) };
    }
    case "/invoke/steerTurn": {
      if (body.input === undefined) {
        throw new HttpError(400, "input is required");
      }
      const params = mergeParams(body, {
        ...(body.threadId ? { threadId: body.threadId } : {}),
        ...(body.turnId ? { turnId: body.turnId } : {}),
        input: normalizeInput(body.input),
      });
      const result = await client.request("turn/steer", params, body.timeoutMs);
      return { result, recentEvents: client.recentEvents({ limit: 50 }) };
    }
    case "/invoke/interruptTurn": {
      const params = mergeParams(body, {
        ...(body.threadId ? { threadId: body.threadId } : {}),
        ...(body.turnId ? { turnId: body.turnId } : {}),
      });
      const result = await client.request("turn/interrupt", params, body.timeoutMs);
      return { result, recentEvents: client.recentEvents({ limit: 50 }) };
    }
    case "/invoke/recentEvents":
      return client.recentEvents(body);
    case "/invoke/request": {
      if (!body.method) {
        throw new HttpError(400, "method is required");
      }
      const result = await client.request(body.method, body.params || {}, body.timeoutMs);
      return { result, recentEvents: client.recentEvents({ limit: 50 }) };
    }
    default:
      throw new HttpError(404, `unknown invoke path: ${pathname}`);
  }
}

async function startServer(options) {
  const resolved = serverOptions(options);
  if (options.daemon) {
    daemonize(resolved);
    return;
  }

  const client = new CodexAppServerClient(resolved);
  let shuttingDown = false;
  const server = createServer(async (request, response) => {
    const url = new URL(request.url || "/", `http://${request.headers.host || `${resolved.host}:${resolved.port}`}`);
    try {
      if (request.method === "GET" && url.pathname === "/healthz") {
        writeJson(response, 200, { ok: true, status: client.status() });
        return;
      }
      if (
        request.method === "POST"
        && url.pathname === "/__shutdown"
        && process.env.CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN === "1"
      ) {
        writeJson(response, 200, { ok: true });
        setImmediate(shutdown);
        return;
      }
      if (request.method === "POST" && url.pathname.startsWith("/invoke/")) {
        const body = await readJsonRequest(request);
        const data = await handleInvoke(url.pathname, body, client);
        writeJson(response, 200, { ok: true, data });
        return;
      }
      throw new HttpError(404, "not found");
    } catch (error) {
      const statusCode = error.statusCode || 500;
      writeJson(response, statusCode, {
        ok: false,
        error: {
          message: error.message,
          code: error.code,
          data: error.data,
        },
      });
    }
  });

  await new Promise((resolvePromise, rejectPromise) => {
    server.once("error", rejectPromise);
    server.listen(resolved.port, resolved.host, resolvePromise);
  });
  console.log(JSON.stringify({
    ok: true,
    url: `http://${resolved.host}:${resolved.port}`,
    pid: process.pid,
  }));

  const shutdown = async () => {
    if (shuttingDown) {
      return;
    }
    shuttingDown = true;
    await client.shutdown();
    server.closeAllConnections?.();
    server.close(() => process.exit(0));
    setTimeout(() => process.exit(0), 1000).unref();
  };
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

function printHelp() {
  console.log(`baijimu-connector-codex ${VERSION}

Usage:
  baijimu-connector-codex start [--host 127.0.0.1] [--port 18110] [--codex-binary codex] [--listen stdio://] [--daemon]
  baijimu-connector-codex status
  baijimu-connector-codex stop
  baijimu-connector-codex --version

Environment:
  CODEX_CONNECTOR_PORT=18110
  CODEX_CONNECTOR_CODEX_BINARY=codex
  CODEX_CONNECTOR_CODEX_ARGS='["app-server","--listen","stdio://"]'
`);
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.version || options.command === "--version") {
    console.log(VERSION);
    return;
  }
  if (options.help || options.command === "help") {
    printHelp();
    return;
  }

  if (options.command === "start") {
    await startServer(options);
    return;
  }

  if (options.command === "status") {
    const path = pidPath();
    console.log(JSON.stringify({
      pidPath: path,
      pid: existsSync(path) ? readFileSync(path, "utf8").trim() : null,
      logPath: logPath(),
    }, null, 2));
    return;
  }

  if (options.command === "stop") {
    const path = pidPath();
    if (!existsSync(path)) {
      console.log(JSON.stringify({ ok: true, stopped: false, reason: "pid file not found" }));
      return;
    }
    const pid = Number(readFileSync(path, "utf8").trim());
    process.kill(pid, "SIGTERM");
    console.log(JSON.stringify({ ok: true, stopped: true, pid }));
    return;
  }

  throw new HttpError(2, `unknown command: ${options.command}`);
}

main().catch((error) => {
  console.error(error.message);
  process.exit(error.statusCode === 2 ? 2 : 1);
});
