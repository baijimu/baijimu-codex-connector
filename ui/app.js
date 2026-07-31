import {
  DEFAULT_MODEL,
  codexTurnMessages,
  credentialStatusMeta,
  formatCodexSessionTime,
  normalizeCodexProjects,
  normalizeCodexSessions,
  normalizeCredentialState,
} from "./state.mjs";

const elementIds = [
  "refresh-button", "message", "error", "warning", "sessions-view", "account-view",
  "credential-badge", "active-workspace", "active-model", "codex-configured",
  "setup-status", "setup-message", "setup-retry-button",
  "new-session-button", "session-project-filter", "session-list", "load-more-sessions",
  "session-path", "session-title", "session-status", "conversation", "prompt-form",
  "session-cwd", "session-project-options", "session-model", "prompt-input", "prompt-hint",
  "interrupt-button", "send-button",
];
const elements = Object.fromEntries(elementIds.map((id) => [id, document.getElementById(id)]));

let activeView = "sessions";
let credentialState = null;
let setupState = null;
let setupMonitorGeneration = 0;
let codexProjects = [];
let sessions = [];
let nextSessionCursor = null;
let selectedSessionId = "";
let selectedTurnId = "";
let sessionBusy = false;
let eventSequence = 0;
let monitorGeneration = 0;

function bridge() {
  const api = window.baijimuLocalApp;
  if (!api || api.version !== 1 || typeof api.invoke !== "function") {
    throw new Error("当前 Bridge Agent 不支持应用内嵌界面，请先升级 Bridge Agent。");
  }
  return api;
}

function setMessage(target, value) {
  elements[target].textContent = value;
  elements[target].hidden = !value;
}

function clearNotices() {
  setMessage("message", "");
  setMessage("error", "");
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error || "操作失败");
}

function option(value, label) {
  const item = document.createElement("option");
  item.value = String(value);
  item.textContent = label;
  return item;
}

function switchView(view) {
  activeView = view;
  elements["sessions-view"].hidden = view !== "sessions";
  elements["account-view"].hidden = view !== "account";
  document.querySelectorAll("[data-view]").forEach((button) => {
    button.classList.toggle("active", button.dataset.view === view);
  });
  if (view === "sessions" && sessions.length === 0) void loadSessions();
  if (view === "account" && !credentialState) void loadState();
}

function setAccountBusy(value) {
  elements["setup-retry-button"].disabled = value;
  elements["setup-retry-button"].textContent = value ? "正在安装配置…" : "重试安装配置";
}

function renderCredentialState() {
  const state = credentialState;
  const active = state?.activeProfile;
  const status = credentialStatusMeta(state?.credentialStatus);
  elements["credential-badge"].textContent = status.label;
  elements["credential-badge"].className = `status-badge ${status.tone}`;
  const currentWorkspaceId = state?.currentWorkspaceId || active?.workspaceId;
  const currentWorkspace = state?.workspaces.find((item) => item.workspaceId === currentWorkspaceId);
  elements["active-workspace"].textContent = currentWorkspaceId
    ? `${currentWorkspace?.name || active?.workspaceName || `工作区 ${currentWorkspaceId}`}（${currentWorkspaceId}）`
    : "尚未识别";
  elements["active-model"].textContent = active?.model || DEFAULT_MODEL;
  elements["codex-configured"].textContent = state?.codexConfigured ? "已由本地应用管理" : "尚未完成管理配置";
  setMessage("warning", state?.discoveryWarning || "");
  if (active) {
    elements["session-model"].value = active.model;
  }
}

function renderSetupState() {
  const status = String(setupState?.status || "pending");
  const labels = {
    pending: "等待安装",
    running: "正在安装配置",
    succeeded: "已完成",
    failed: "安装失败",
  };
  elements["setup-status"].textContent = labels[status] || status;
  elements["setup-message"].textContent = setupState?.error || setupState?.message || "等待安装";
  setAccountBusy(status === "running");
}

async function monitorSetup() {
  const generation = ++setupMonitorGeneration;
  while (generation === setupMonitorGeneration && setupState?.status === "running") {
    await new Promise((resolve) => window.setTimeout(resolve, 1000));
    if (generation !== setupMonitorGeneration) return;
    try {
      setupState = await bridge().invoke("setupState");
      renderSetupState();
      if (setupState?.status === "succeeded") {
        await loadState("Codex 已在本机安装并配置完成。");
        return;
      }
      if (setupState?.status === "failed") {
        setMessage("error", setupState?.error || "Codex 安装配置失败。");
        return;
      }
    } catch (error) {
      setAccountBusy(false);
      setMessage("error", errorMessage(error));
      return;
    }
  }
}

async function loadState(successMessage = "") {
  clearNotices();
  setAccountBusy(true);
  try {
    const [credential, setup] = await Promise.all([
      bridge().invoke("credentialState"),
      bridge().invoke("setupState"),
    ]);
    credentialState = normalizeCredentialState(credential);
    setupState = setup;
    renderCredentialState();
    renderSetupState();
    if (setupState?.status === "running") void monitorSetup();
    if (successMessage) setMessage("message", successMessage);
  } catch (error) {
    credentialState = null;
    setMessage("error", errorMessage(error));
    setAccountBusy(false);
  }
}

async function retrySetup() {
  clearNotices();
  const workspaceId = credentialState?.currentWorkspaceId;
  if (!workspaceId) return setMessage("error", "客户端当前授权中缺少工作区信息。");
  setAccountBusy(true);
  try {
    setupState = await bridge().invoke("setupRetry", { workspaceId });
    renderSetupState();
    setMessage("message", "已开始在本机安装并配置 Codex。");
    void monitorSetup();
  } catch (error) {
    setMessage("error", errorMessage(error));
    setAccountBusy(false);
  }
}

function renderCodexProjects() {
  const currentFilter = elements["session-project-filter"].value;
  elements["session-project-filter"].replaceChildren(option("", "全部项目"));
  elements["session-project-options"].replaceChildren();
  codexProjects.forEach((project) => {
    elements["session-project-filter"].append(option(project.path, project.title));
    elements["session-project-options"].append(option(project.path, project.title));
  });
  if (codexProjects.some((project) => project.path === currentFilter)) elements["session-project-filter"].value = currentFilter;
}

function renderSessionList() {
  const cwd = elements["session-project-filter"].value;
  const visible = cwd ? sessions.filter((session) => session.cwd === cwd) : sessions;
  elements["session-list"].replaceChildren();
  if (!visible.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "没有符合条件的会话。";
    elements["session-list"].append(empty);
  }
  visible.forEach((session) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `session-item${selectedSessionId === session.id ? " active" : ""}`;
    const title = document.createElement("strong");
    title.textContent = session.name;
    const preview = document.createElement("span");
    preview.textContent = session.preview || session.cwd || "暂无摘要";
    const time = document.createElement("small");
    time.textContent = formatCodexSessionTime(session);
    button.append(title, preview, time);
    button.addEventListener("click", () => void openSession(session));
    elements["session-list"].append(button);
  });
  elements["load-more-sessions"].hidden = !nextSessionCursor;
}

function renderConversation(turns) {
  const messages = codexTurnMessages(turns);
  elements.conversation.replaceChildren();
  if (!messages.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = selectedSessionId ? "这个会话还没有可显示的消息。" : "输入第一条开发指令开始新会话。";
    elements.conversation.append(empty);
    return;
  }
  messages.forEach((message) => {
    const article = document.createElement("article");
    article.className = `chat-message ${message.role}`;
    const role = document.createElement("strong");
    role.textContent = message.role === "user" ? "你" : message.role === "assistant" ? "Codex" : "系统";
    const text = document.createElement("pre");
    text.textContent = message.text;
    article.append(role, text);
    elements.conversation.append(article);
  });
  elements.conversation.scrollTop = elements.conversation.scrollHeight;
}

function setSessionBusy(value, status = value ? "执行中" : "就绪") {
  sessionBusy = value;
  elements["send-button"].disabled = value;
  elements["session-cwd"].disabled = value || Boolean(selectedSessionId);
  elements["session-model"].disabled = value;
  elements["new-session-button"].disabled = value;
  elements["interrupt-button"].hidden = !value;
  elements["session-status"].textContent = status;
  elements["session-status"].className = `status-badge ${value ? "warning" : "success"}`;
  elements["send-button"].textContent = value ? "Codex 正在执行…" : "发送";
}

async function loadSessions({ append = false } = {}) {
  clearNotices();
  try {
    if (!append) {
      const projectsResponse = await bridge().invoke("listCodexProjects", { limit: 100, includeThreadStats: false });
      codexProjects = normalizeCodexProjects(projectsResponse?.result);
      renderCodexProjects();
    }
    const response = await bridge().invoke("listCodexSessions", {
      limit: 50,
      cursor: append ? nextSessionCursor : null,
      sortKey: "updated_at",
      sortDirection: "desc",
      archived: false,
    });
    const page = normalizeCodexSessions(response?.result);
    sessions = append ? normalizeCodexSessions([...sessions, ...page]) : page;
    nextSessionCursor = response?.result?.nextCursor || null;
    renderSessionList();
  } catch (error) {
    setMessage("error", errorMessage(error));
    elements["session-list"].replaceChildren();
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "暂时无法读取 Codex 会话。";
    elements["session-list"].append(empty);
  }
}

async function readTurns(threadId) {
  const response = await bridge().invoke("listCodexTurns", {
    threadId,
    limit: 100,
    sortDirection: "asc",
    itemsView: "full",
  });
  const turns = Array.isArray(response?.result?.data) ? response.result.data : [];
  renderConversation(turns);
  return turns;
}

async function openSession(session) {
  monitorGeneration += 1;
  selectedSessionId = session.id;
  selectedTurnId = "";
  elements["session-cwd"].value = session.cwd;
  elements["session-title"].textContent = session.name;
  elements["session-path"].textContent = session.cwd || "目录未知";
  elements["prompt-hint"].textContent = "继续发送指令会在当前会话中启动新的 turn。";
  setSessionBusy(false);
  renderSessionList();
  try {
    await readTurns(session.id);
  } catch (error) {
    setMessage("error", errorMessage(error));
  }
}

function newSession() {
  monitorGeneration += 1;
  selectedSessionId = "";
  selectedTurnId = "";
  elements["session-title"].textContent = "新建会话";
  elements["session-path"].textContent = "选择项目目录后发送第一条指令";
  elements["session-cwd"].value = elements["session-project-filter"].value || codexProjects[0]?.path || "";
  elements["prompt-input"].value = "";
  elements["prompt-hint"].textContent = "发送首条指令时会创建会话并立即开始执行。";
  renderConversation([]);
  renderSessionList();
  setSessionBusy(false);
  elements["prompt-input"].focus();
}

function eventMatchesTurn(event, threadId, turnId) {
  const params = event?.params || {};
  const eventTurnId = params?.turnId || params?.turn?.id;
  const eventThreadId = params?.threadId || params?.thread?.id;
  return (!eventTurnId || eventTurnId === turnId) && (!eventThreadId || eventThreadId === threadId);
}

async function monitorTurn(threadId, turnId, generation) {
  const deadline = Date.now() + 30 * 60 * 1000;
  while (generation === monitorGeneration && Date.now() < deadline) {
    await new Promise((resolve) => window.setTimeout(resolve, 1200));
    if (generation !== monitorGeneration) return;
    try {
      const response = await bridge().invoke("recentCodexEvents", { afterSequence: eventSequence, limit: 200 });
      const events = Array.isArray(response?.events) ? response.events : [];
      eventSequence = Math.max(
        eventSequence,
        Number(response?.latestSequence) || 0,
        ...events.map((event) => Number(event?.sequence) || 0),
      );
      await readTurns(threadId);
      const terminal = events.find((event) => eventMatchesTurn(event, threadId, turnId)
        && ["turn/completed", "turn/failed", "turn/cancelled", "turn/interrupted"].includes(event.method));
      if (terminal) {
        setSessionBusy(false, terminal.method === "turn/completed" ? "已完成" : "已停止");
        await loadSessions();
        return;
      }
    } catch (error) {
      setSessionBusy(false, "读取失败");
      setMessage("error", errorMessage(error));
      return;
    }
  }
  if (generation === monitorGeneration) {
    setSessionBusy(false, "仍在后台执行");
    setMessage("warning", "会话仍在后台运行，请稍后刷新查看结果。");
  }
}

async function sendPrompt(event) {
  event.preventDefault();
  clearNotices();
  const cwd = elements["session-cwd"].value.trim();
  const model = elements["session-model"].value.trim();
  const prompt = elements["prompt-input"].value.trim();
  if (!cwd || !model || !prompt || sessionBusy) return;
  const generation = ++monitorGeneration;
  setSessionBusy(true);
  try {
    if (!selectedSessionId) {
      const started = await bridge().invoke("startCodexSession", { cwd, model });
      selectedSessionId = String(started?.result?.thread?.id || "");
      if (!selectedSessionId) throw new Error("Codex 没有返回新会话 ID。");
      elements["session-title"].textContent = "新会话";
      elements["session-path"].textContent = cwd;
      elements["session-cwd"].disabled = true;
    }
    const startedTurn = await bridge().invoke("startCodexTurn", {
      threadId: selectedSessionId,
      input: prompt,
      cwd,
      model,
    });
    selectedTurnId = String(startedTurn?.result?.turn?.id || "");
    elements["prompt-input"].value = "";
    elements["prompt-hint"].textContent = "Codex 正在本机项目中执行；完成后可以继续发送。";
    await readTurns(selectedSessionId);
    void monitorTurn(selectedSessionId, selectedTurnId, generation);
  } catch (error) {
    setSessionBusy(false, "执行失败");
    setMessage("error", errorMessage(error));
  }
}

async function interruptTurn() {
  if (!sessionBusy || !selectedSessionId) return;
  try {
    await bridge().invoke("interruptCodexTurn", { threadId: selectedSessionId, turnId: selectedTurnId || null });
    monitorGeneration += 1;
    setSessionBusy(false, "已停止");
    await readTurns(selectedSessionId);
  } catch (error) {
    setMessage("error", errorMessage(error));
  }
}

document.querySelectorAll("[data-view]").forEach((button) => button.addEventListener("click", () => switchView(button.dataset.view)));
elements["refresh-button"].addEventListener("click", () => activeView === "sessions" ? void loadSessions() : void loadState("状态已刷新。"));
elements["new-session-button"].addEventListener("click", newSession);
elements["session-project-filter"].addEventListener("change", renderSessionList);
elements["load-more-sessions"].addEventListener("click", () => void loadSessions({ append: true }));
elements["prompt-form"].addEventListener("submit", (event) => void sendPrompt(event));
elements["interrupt-button"].addEventListener("click", () => void interruptTurn());
elements["setup-retry-button"].addEventListener("click", () => void retrySetup());

void Promise.all([loadSessions(), loadState()]);
