import {
  DEFAULT_MODEL,
  credentialStatusMeta,
  normalizeCredentialState,
  normalizeSetupProgress,
  shouldShowSetupProgress,
} from "./state.mjs";

const elementIds = [
  "refresh-button", "open-codex-button", "message", "error", "warning",
  "credential-badge", "active-workspace", "active-codex-home", "active-model",
  "codex-configured", "auth-mode", "auth-profile-list", "setup-status",
  "setup-message", "setup-retry-button", "setup-progress", "setup-progress-label",
  "setup-progress-percent", "setup-progress-track", "setup-progress-bar", "setup-step-list",
  "switch-progress", "switch-progress-message", "auth-switch-modal",
  "auth-switch-modal-title", "auth-switch-modal-message", "auth-switch-cancel",
  "auth-switch-confirm",
];
const elements = Object.fromEntries(elementIds.map((id) => [id, document.getElementById(id)]));

let credentialState = null;
let setupState = null;
let setupMonitorGeneration = 0;
let pendingCodexLaunch = null;
let accountBusy = false;

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

function setAccountBusy(value) {
  accountBusy = value;
  elements["refresh-button"].disabled = value;
  elements["open-codex-button"].disabled = value;
  elements["setup-retry-button"].disabled = value;
  elements["setup-retry-button"].textContent = value ? "正在处理…" : "重新安装并修复";
  document.querySelectorAll(".auth-switch").forEach((button) => {
    button.disabled = value || button.dataset.profileDisabled === "true";
  });
}

function profileRow({ title, detail, meta, active, disabled, actionLabel, onSwitch }) {
  const row = document.createElement("div");
  row.className = `profile-row${active ? " active" : ""}${disabled ? " unavailable" : ""}`;
  const copy = document.createElement("div");
  copy.className = "profile-copy";
  const heading = document.createElement("div");
  heading.className = "profile-heading";
  const strong = document.createElement("strong");
  strong.textContent = title;
  heading.append(strong);
  if (active) {
    const current = document.createElement("span");
    current.className = "current-label";
    current.textContent = "当前环境";
    heading.append(current);
  }
  const span = document.createElement("span");
  span.textContent = detail;
  const small = document.createElement("small");
  small.textContent = meta;
  copy.append(heading, span, small);
  const button = document.createElement("button");
  button.type = "button";
  button.className = `button ${active ? "secondary" : "primary"} compact auth-switch`;
  button.textContent = actionLabel || (active ? "重新启动 Codex" : "启动 Codex");
  button.dataset.profileDisabled = String(disabled);
  button.disabled = accountBusy || disabled;
  button.addEventListener("click", onSwitch);
  row.append(copy, button);
  return row;
}

function authProfiles() {
  const state = credentialState;
  const profiles = [{
    key: "chatgpt",
    title: "原有 Codex 环境",
    detail: state?.chatgpt?.accountId ? `ChatGPT 账号 ${state.chatgpt.accountId}` : "接管前的默认 Codex 登录",
    meta: state?.chatgpt?.configured ? "已配置；恢复后使用原有登录、配置和会话" : "尚未登录；恢复后可在 Codex 中完成登录",
    active: state?.activeMode === "chatgpt",
    disabled: false,
    actionLabel: state?.activeMode === "chatgpt" ? "重新启动个人 Codex" : "启动个人 Codex",
    request: { mode: "chatgpt" },
  }];
  for (const workspace of state?.workspaces || []) {
    const profile = state.profiles.find((item) => item.workspaceId === workspace.workspaceId);
    const active = state.activeMode === "baijimu" && state.activeWorkspaceId === workspace.workspaceId;
    profiles.push({
      key: `workspace-${workspace.workspaceId}`,
      title: `${workspace.name || `工作区 ${workspace.workspaceId}`}（${workspace.workspaceId}）`,
      detail: `${profile?.environment || "prod"} 环境`,
      meta: workspace.authorized
        ? (profile ? "凭证档案已保存；可以直接启动 Codex" : "已授权；首次启动时自动创建工作区凭证档案")
        : "当前百积木账号未授权这个工作区",
      active,
      disabled: !workspace.authorized,
      actionLabel: active ? "重新启动 Codex" : "启动 Codex",
      request: { mode: "baijimu", workspaceId: workspace.workspaceId },
    });
  }
  return profiles.sort((left, right) => {
    if (left.active !== right.active) return left.active ? -1 : 1;
    if (left.disabled !== right.disabled) return left.disabled ? 1 : -1;
    return 0;
  });
}

function renderAuthProfiles() {
  const list = elements["auth-profile-list"];
  list.replaceChildren();
  for (const profile of authProfiles()) {
    list.append(profileRow({
      ...profile,
      onSwitch: () => openAuthSwitchModal(profile.request),
    }));
  }
  if (!list.children.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "没有可用的 Codex 环境。";
    list.append(empty);
  }
}

function renderCredentialState() {
  const state = credentialState;
  const active = state?.activeProfile;
  const status = credentialStatusMeta(state?.credentialStatus);
  const currentWorkspaceId = state?.activeWorkspaceId || active?.workspaceId;
  const currentWorkspace = state?.workspaces.find((item) => item.workspaceId === currentWorkspaceId);
  elements["credential-badge"].textContent = status.label;
  elements["credential-badge"].className = `status-badge ${status.tone}`;
  elements["auth-mode"].textContent = state?.activeMode === "baijimu" ? "百积木工作区凭证" : "原有 ChatGPT 登录";
  elements["active-workspace"].textContent = state?.activeMode === "chatgpt"
    ? "原有 Codex 环境"
    : currentWorkspaceId
      ? `${currentWorkspace?.name || active?.workspaceName || `工作区 ${currentWorkspaceId}`}（${currentWorkspaceId}）`
      : "尚未识别";
  const activeHome = state?.activeCodexHome || active?.codexHome || state?.originalCodexHome;
  elements["active-codex-home"].textContent = activeHome || "使用 Codex 默认目录";
  elements["active-codex-home"].title = activeHome || "";
  elements["active-model"].textContent = active?.model || DEFAULT_MODEL;
  elements["codex-configured"].textContent = state?.codexConfigured ? "已由 Connector 管理" : "尚未完成配置";
  setMessage("warning", state?.discoveryWarning || "");
  renderAuthProfiles();
}

function codexLaunchCopy(request) {
  const currentName = elements["active-workspace"].textContent || "当前环境";
  if (request.mode === "chatgpt") {
    return {
      title: "启动个人 Codex",
      message: `将关闭当前 Codex，并使用“${currentName}”接管前的个人状态目录重新启动。不会删除任何工作区目录。`,
      progress: "正在使用个人状态目录启动 Codex…",
    };
  }
  const workspace = credentialState?.workspaces?.find(
    (item) => item.workspaceId === Number(request.workspaceId),
  );
  const name = workspace?.name
    ? `${workspace.name}（${workspace.workspaceId}）`
    : `工作区 ${request.workspaceId}`;
  return {
    title: `使用${name}启动 Codex`,
    message: `将关闭当前 Codex，选择“${name}”的独立状态目录并重新启动。不会删除个人或其他工作区数据。`,
    progress: `正在使用${name}启动并验证 Codex…`,
  };
}

function closeAuthSwitchModal() {
  pendingCodexLaunch = null;
  elements["auth-switch-modal"].hidden = true;
}

function openAuthSwitchModal(request) {
  if (pendingCodexLaunch || accountBusy) return;
  const copy = codexLaunchCopy(request);
  pendingCodexLaunch = request;
  elements["auth-switch-modal-title"].textContent = copy.title;
  elements["auth-switch-modal-message"].textContent = copy.message;
  elements["auth-switch-modal"].hidden = false;
  elements["auth-switch-confirm"].focus();
}

async function confirmAuthSwitch() {
  const request = pendingCodexLaunch;
  if (!request) return;
  const copy = codexLaunchCopy(request);
  closeAuthSwitchModal();
  await launchCodex(request, copy.progress);
}

async function launchCodex(request, progressMessage) {
  clearNotices();
  setAccountBusy(true);
  elements["switch-progress-message"].textContent = progressMessage;
  elements["switch-progress"].hidden = false;
  try {
    const response = await bridge().invoke("launchCodex", request);
    credentialState = normalizeCredentialState(response);
    renderCredentialState();
    setMessage(
      "message",
      request.mode === "chatgpt"
        ? "个人 Codex 已启动并验证。"
        : "Codex 已使用所选百积木工作区启动并验证。",
    );
  } catch (error) {
    setMessage("error", errorMessage(error));
    await loadState({ ensureReady: false, monitor: false });
  } finally {
    elements["switch-progress"].hidden = true;
    setAccountBusy(false);
  }
}

function renderSetupState() {
  const status = String(setupState?.status || "pending");
  const labels = {
    pending: "等待初始化",
    running: "正在初始化",
    succeeded: "已完成",
    failed: "初始化失败",
  };
  elements["setup-status"].textContent = labels[status] || status;
  elements["setup-message"].textContent = setupState?.error || setupState?.message || "等待初始化";
  renderSetupProgress();
  setAccountBusy(status === "running");
  elements["setup-retry-button"].hidden = status !== "failed";
  elements["setup-retry-button"].textContent = "重新安装并修复";
}

function formatBytes(value) {
  const bytes = Math.max(0, Number(value) || 0);
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}

function setupStepStateLabel(state) {
  return ({
    pending: "等待",
    running: "进行中",
    completed: "完成",
    skipped: "跳过",
    failed: "失败",
  })[state] || state;
}

function renderSetupProgress() {
  const progress = normalizeSetupProgress(setupState);
  const visible = shouldShowSetupProgress(setupState);
  elements["setup-progress"].hidden = !visible;
  if (!visible) return;

  const current = progress.steps.find((step) => step.state === "running")
    || [...progress.steps].reverse().find((step) => ["completed", "failed"].includes(step.state));
  elements["setup-progress-label"].textContent = current
    ? `${current.index}/${progress.steps.length} ${current.name}`
    : "准备初始化";
  elements["setup-progress-percent"].textContent = `${progress.percent}%`;
  elements["setup-progress-track"].setAttribute("aria-valuenow", String(progress.percent));
  elements["setup-progress-bar"].style.width = `${progress.percent}%`;

  const list = elements["setup-step-list"];
  list.replaceChildren();
  progress.steps.forEach((step) => {
    const item = document.createElement("li");
    item.className = `setup-step ${step.state}`;
    const marker = document.createElement("span");
    marker.className = "setup-step-marker";
    marker.textContent = ["completed", "skipped"].includes(step.state) ? "✓" : String(step.index);
    const copy = document.createElement("span");
    copy.className = "setup-step-copy";
    const title = document.createElement("strong");
    title.textContent = step.name;
    const detail = document.createElement("small");
    const download = step.totalBytes > 0 && step.downloadedBytes != null
      ? ` · ${formatBytes(step.downloadedBytes)} / ${formatBytes(step.totalBytes)}`
      : "";
    detail.textContent = `${step.detail || setupStepStateLabel(step.state)}${download}`;
    const state = document.createElement("em");
    state.textContent = setupStepStateLabel(step.state);
    copy.append(title, detail);
    item.append(marker, copy, state);
    list.append(item);
  });
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
        await loadState({ ensureReady: false, monitor: false });
        setMessage("message", "本机 Codex 已完成安装配置，可以切换工作区或直接打开 Codex。");
        return;
      }
      if (setupState?.status === "failed") {
        setMessage("error", setupState?.error || "Codex 初始化失败。");
        return;
      }
    } catch (error) {
      setAccountBusy(false);
      setMessage("error", errorMessage(error));
      return;
    }
  }
}

async function ensureCodexReady() {
  const readiness = await bridge().invoke("ensureCodexReady", {});
  setupState = readiness?.setup || setupState;
  renderSetupState();
  switch (readiness?.readiness) {
    case "ready":
      return;
    case "initializing":
      setMessage("message", readiness?.message || "正在自动下载安装并配置本机 Codex。");
      void monitorSetup();
      return;
    case "failed":
      setMessage("error", readiness?.message || "Codex 初始化失败，请检查失败步骤后重新安装修复。");
      return;
    case "needs_workspace":
      setMessage("error", readiness?.message || "请先完成当前百积木工作区授权。");
      return;
    default:
      throw new Error(readiness?.message || "无法确认本机 Codex 初始化状态。");
  }
}

async function loadState({ ensureReady = false, monitor = true, successMessage = "" } = {}) {
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
    if (ensureReady) await ensureCodexReady();
    else if (monitor && setupState?.status === "running") void monitorSetup();
    if (successMessage) setMessage("message", successMessage);
  } catch (error) {
    setMessage("error", errorMessage(error));
  } finally {
    if (setupState?.status !== "running") setAccountBusy(false);
  }
}

async function retrySetup() {
  clearNotices();
  const workspaceId = credentialState?.currentWorkspaceId;
  if (!workspaceId) {
    setMessage("error", "客户端当前授权中缺少工作区信息。");
    return;
  }
  setAccountBusy(true);
  try {
    setupState = await bridge().invoke("setupRetry", { workspaceId });
    renderSetupState();
    setMessage("message", "已开始重新安装并修复本机 Codex。");
    void monitorSetup();
  } catch (error) {
    setAccountBusy(false);
    setMessage("error", errorMessage(error));
  }
}

async function openCodex() {
  clearNotices();
  setAccountBusy(true);
  elements["open-codex-button"].textContent = "正在打开…";
  try {
    const request = credentialState?.activeMode === "baijimu" && credentialState?.activeWorkspaceId
      ? { mode: "baijimu", workspaceId: credentialState.activeWorkspaceId }
      : { mode: "chatgpt" };
    await launchCodex(request, "正在重新启动并验证当前 Codex 环境…");
  } catch (error) {
    setMessage("error", errorMessage(error));
  } finally {
    elements["open-codex-button"].textContent = "打开 Codex";
    setAccountBusy(false);
  }
}

elements["refresh-button"].addEventListener("click", () => void loadState({
  ensureReady: true,
  successMessage: "工作区状态已刷新。",
}));
elements["open-codex-button"].addEventListener("click", () => void openCodex());
elements["setup-retry-button"].addEventListener("click", () => void retrySetup());
elements["auth-switch-cancel"].addEventListener("click", closeAuthSwitchModal);
elements["auth-switch-confirm"].addEventListener("click", () => void confirmAuthSwitch());
elements["auth-switch-modal"].addEventListener("click", (event) => {
  if (event.target === elements["auth-switch-modal"]) closeAuthSwitchModal();
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && pendingCodexLaunch) closeAuthSwitchModal();
});

void loadState({ ensureReady: true });
