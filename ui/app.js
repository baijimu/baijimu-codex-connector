import {
  DEFAULT_MODEL,
  buildSwitchPayload,
  codexTurnMessages,
  credentialStatusMeta,
  formatActivatedAt,
  formatCodexSessionTime,
  normalizeCodexProjects,
  normalizeCodexSessions,
  normalizeCredentialState,
  preferredWorkspaceId,
  projectLabel,
} from "./state.mjs";

const elementIds = [
  "refresh-button", "message", "error", "warning", "sessions-view", "account-view",
  "credential-badge", "active-workspace", "active-project", "active-model", "codex-configured",
  "switch-form", "workspace-select", "project-id", "project-name", "project-options",
  "project-hint", "model", "checkout-button", "switch-button", "profile-count", "profile-list",
  "new-session-button", "session-project-filter", "session-list", "load-more-sessions",
  "session-path", "session-title", "session-status", "conversation", "prompt-form",
  "session-cwd", "session-project-options", "session-model", "prompt-input", "prompt-hint",
  "interrupt-button", "send-button",
];
const elements = Object.fromEntries(elementIds.map((id) => [id, document.getElementById(id)]));

let activeView = "sessions";
let credentialState = null;
let workspaceProjects = [];
let accountBusy = false;
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
  accountBusy = value;
  elements["workspace-select"].disabled = value || !credentialState?.workspaces.length;
  elements["project-id"].disabled = value || !credentialState;
  elements["project-name"].disabled = value || !credentialState;
  elements.model.disabled = value || !credentialState;
  elements["checkout-button"].disabled = value || !credentialState;
  elements["switch-button"].disabled = value || !credentialState;
  document.querySelectorAll("[data-profile-index]").forEach((button) => {
    button.disabled = value || button.dataset.active === "true";
  });
  elements["switch-button"].textContent = value ? "正在切换…" : "签发并切换";
  elements["checkout-button"].textContent = value ? "正在检出…" : "检出项目源码";
}

function renderCredentialState() {
  const state = credentialState;
  const active = state?.activeProfile;
  const status = credentialStatusMeta(state?.credentialStatus);
  elements["credential-badge"].textContent = status.label;
  elements["credential-badge"].className = `status-badge ${status.tone}`;
  elements["active-workspace"].textContent = active ? `${active.workspaceName}（${active.workspaceId}）` : "尚未识别";
  elements["active-project"].textContent = active ? `${projectLabel(active)}（${active.projectId}）` : "尚未识别";
  elements["active-model"].textContent = active?.model || DEFAULT_MODEL;
  elements["codex-configured"].textContent = state?.codexConfigured ? "已由本地应用管理" : "尚未完成管理配置";
  setMessage("warning", state?.discoveryWarning || "");
  const selectedWorkspaceId = preferredWorkspaceId(state, elements["workspace-select"].value);
  elements["workspace-select"].replaceChildren(option("", "选择工作区"));
  state.workspaces.forEach((workspace) => {
    elements["workspace-select"].append(option(workspace.workspaceId, `${workspace.name}（${workspace.workspaceId}）`));
  });
  elements["workspace-select"].value = selectedWorkspaceId ? String(selectedWorkspaceId) : "";
  if (active && active.workspaceId === selectedWorkspaceId) {
    elements["project-id"].value = String(active.projectId);
    elements["project-name"].value = active.projectName;
    elements.model.value = active.model;
    elements["session-model"].value = active.model;
  }
  renderProfiles();
  setAccountBusy(false);
}

function renderProfiles() {
  const profiles = credentialState?.profiles || [];
  const active = credentialState?.activeProfile;
  elements["profile-count"].textContent = `${profiles.length} 个配置`;
  elements["profile-list"].replaceChildren();
  if (!profiles.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "还没有由本地应用管理的工作区配置。";
    elements["profile-list"].append(empty);
    return;
  }
  profiles.forEach((profile, index) => {
    const isActive = active?.workspaceId === profile.workspaceId && active?.projectId === profile.projectId;
    const row = document.createElement("article");
    row.className = `profile-row${isActive ? " active" : ""}`;
    const copy = document.createElement("div");
    copy.className = "profile-copy";
    const title = document.createElement("strong");
    title.textContent = profile.workspaceName;
    const detail = document.createElement("span");
    detail.textContent = `${projectLabel(profile)} · ${profile.model}`;
    const time = document.createElement("small");
    time.textContent = formatActivatedAt(profile.activatedAtEpochSeconds);
    copy.append(title, detail, time);
    const button = document.createElement("button");
    button.type = "button";
    button.className = `button ${isActive ? "secondary" : "primary compact"}`;
    button.textContent = isActive ? "当前使用" : "切换";
    button.dataset.profileIndex = String(index);
    button.dataset.active = String(isActive);
    button.disabled = accountBusy || isActive;
    button.addEventListener("click", () => void switchCredential(profile));
    row.append(copy, button);
    elements["profile-list"].append(row);
  });
}

function renderWorkspaceProjects() {
  elements["project-options"].replaceChildren();
  workspaceProjects.forEach((project) => elements["project-options"].append(option(project.projectId, project.name)));
  elements["project-hint"].textContent = workspaceProjects.length
    ? `已读取 ${workspaceProjects.length} 个项目；可选择或直接输入项目 ID。`
    : "没有读取到项目；可以直接输入有效的项目 ID。";
}

async function loadWorkspaceProjects(workspaceId, preserveProject = false) {
  workspaceProjects = [];
  renderWorkspaceProjects();
  if (!preserveProject) {
    elements["project-id"].value = "";
    elements["project-name"].value = "";
  }
  if (!workspaceId) return;
  elements["project-hint"].textContent = "正在读取项目列表…";
  try {
    const result = await bridge().invoke("listWorkspaceProjects", { workspaceId });
    workspaceProjects = Array.isArray(result) ? result : [];
    renderWorkspaceProjects();
  } catch (error) {
    elements["project-hint"].textContent = `读取项目失败：${errorMessage(error)}`;
  }
}

async function loadState(successMessage = "") {
  clearNotices();
  setAccountBusy(true);
  try {
    credentialState = normalizeCredentialState(await bridge().invoke("credentialState"));
    renderCredentialState();
    await loadWorkspaceProjects(preferredWorkspaceId(credentialState), true);
    if (successMessage) setMessage("message", successMessage);
  } catch (error) {
    credentialState = null;
    setMessage("error", errorMessage(error));
    setAccountBusy(false);
  }
}

function selectedSwitchPayload() {
  const workspaceId = Number(elements["workspace-select"].value);
  const workspace = credentialState?.workspaces.find((item) => item.workspaceId === workspaceId);
  const projectId = Number(elements["project-id"].value);
  const project = workspaceProjects.find((item) => item.projectId === projectId);
  return buildSwitchPayload({
    workspaceId,
    workspaceName: workspace?.name,
    projectId,
    projectName: elements["project-name"].value.trim() || project?.name,
    model: elements.model.value,
  });
}

async function switchCredential(profile = null) {
  clearNotices();
  let payload;
  try {
    payload = profile ? buildSwitchPayload(profile) : selectedSwitchPayload();
  } catch (error) {
    setMessage("error", errorMessage(error));
    return;
  }
  const project = payload.projectName || `项目 ${payload.projectId}`;
  if (!window.confirm(`将为“${payload.workspaceName}”的“${project}”重新签发 LLM credential，并重启 Codex。继续吗？`)) return;
  setAccountBusy(true);
  try {
    const result = await bridge().invoke("switchCredential", payload);
    credentialState = normalizeCredentialState(result?.state);
    renderCredentialState();
    await loadWorkspaceProjects(payload.workspaceId, true);
    elements["session-model"].value = payload.model;
    setMessage("message", `已切换到 ${payload.workspaceName} / ${project}。${String(result?.restartMessage || "")}`);
  } catch (error) {
    setMessage("error", errorMessage(error));
    setAccountBusy(false);
  }
}

async function checkoutPlatformProject() {
  clearNotices();
  let payload;
  try {
    payload = selectedSwitchPayload();
  } catch (error) {
    setMessage("error", errorMessage(error));
    return;
  }
  setAccountBusy(true);
  try {
    const result = await bridge().invoke("checkoutPlatformProject", {
      workspaceId: payload.workspaceId,
      projectId: payload.projectId,
    });
    const directory = String(result?.directory || "").trim();
    if (!directory) throw new Error("检出成功响应缺少本地目录。");
    const title = payload.projectName || `项目 ${payload.projectId}`;
    codexProjects = [
      { path: directory, title, exists: true, sources: ["platformGitRemote"] },
      ...codexProjects.filter((project) => project.path !== directory),
    ];
    renderCodexProjects();
    elements["session-project-filter"].value = directory;
    switchView("sessions");
    newSession();
    setMessage("message", `${result?.reused ? "已复用" : "已检出"} ${title}：${result?.branch || ""}`);
  } catch (error) {
    setMessage("error", errorMessage(error));
  } finally {
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
elements["workspace-select"].addEventListener("change", () => {
  const workspaceId = Number(elements["workspace-select"].value);
  const active = credentialState?.activeProfile;
  if (active?.workspaceId === workspaceId) {
    elements["project-id"].value = String(active.projectId);
    elements["project-name"].value = active.projectName;
    elements.model.value = active.model;
  } else {
    elements.model.value = DEFAULT_MODEL;
  }
  void loadWorkspaceProjects(workspaceId, active?.workspaceId === workspaceId);
});
elements["project-id"].addEventListener("input", () => {
  const selected = workspaceProjects.find((project) => project.projectId === Number(elements["project-id"].value));
  if (selected) elements["project-name"].value = selected.name;
});
elements["switch-form"].addEventListener("submit", (event) => {
  event.preventDefault();
  void switchCredential();
});
elements["checkout-button"].addEventListener("click", () => void checkoutPlatformProject());

void Promise.all([loadSessions(), loadState()]);
