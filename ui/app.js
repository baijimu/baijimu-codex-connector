import {
  DEFAULT_MODEL,
  buildSwitchPayload,
  credentialStatusMeta,
  formatActivatedAt,
  normalizeCredentialState,
  preferredWorkspaceId,
  projectLabel,
} from "./state.mjs";

const elements = Object.fromEntries(
  [
    "refresh-button", "message", "error", "warning", "credential-badge",
    "active-workspace", "active-project", "active-model", "codex-configured",
    "switch-form", "workspace-select", "project-id", "project-name", "project-options",
    "project-hint", "model", "switch-button", "profile-count", "profile-list",
  ].map((id) => [id, document.getElementById(id)]),
);

let credentialState = null;
let workspaceProjects = [];
let busy = false;

function bridge() {
  const api = window.baijimuLocalApp;
  if (!api || api.version !== 1 || typeof api.invoke !== "function") {
    throw new Error("当前 Bridge Agent 不支持应用内嵌界面，请先升级 Bridge Agent。");
  }
  return api;
}

function setMessage(target, text) {
  elements[target].textContent = text;
  elements[target].hidden = !text;
}

function clearNotices() {
  setMessage("message", "");
  setMessage("error", "");
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error || "操作失败");
}

function setBusy(value) {
  busy = value;
  elements["refresh-button"].disabled = value;
  elements["workspace-select"].disabled = value || !credentialState?.workspaces.length;
  elements["project-id"].disabled = value || !credentialState;
  elements["project-name"].disabled = value || !credentialState;
  elements.model.disabled = value || !credentialState;
  elements["switch-button"].disabled = value || !credentialState;
  document.querySelectorAll("[data-profile-index]").forEach((button) => {
    button.disabled = value || button.dataset.active === "true";
  });
  elements["switch-button"].textContent = value ? "正在切换…" : "签发并切换";
}

function option(value, label) {
  const item = document.createElement("option");
  item.value = String(value);
  item.textContent = label;
  return item;
}

function renderState() {
  const state = credentialState;
  const active = state?.activeProfile;
  const status = credentialStatusMeta(state?.credentialStatus);
  elements["credential-badge"].textContent = status.label;
  elements["credential-badge"].className = `status-badge ${status.tone}`;
  elements["active-workspace"].textContent = active
    ? `${active.workspaceName}（${active.workspaceId}）`
    : "尚未识别";
  elements["active-project"].textContent = active
    ? `${projectLabel(active)}（${active.projectId}）`
    : "尚未识别";
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
  } else if (!elements.model.value.trim()) {
    elements.model.value = DEFAULT_MODEL;
  }
  renderProfiles();
  setBusy(false);
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
    button.disabled = busy || isActive;
    button.addEventListener("click", () => void switchCredential(profile));
    row.append(copy, button);
    elements["profile-list"].append(row);
  });
}

function renderProjects() {
  elements["project-options"].replaceChildren();
  workspaceProjects.forEach((project) => {
    elements["project-options"].append(option(project.projectId, project.name));
  });
  elements["project-hint"].textContent = workspaceProjects.length
    ? `已读取 ${workspaceProjects.length} 个项目；可选择或直接输入项目 ID。`
    : "没有读取到项目；可以直接输入有效的项目 ID。";
}

async function loadProjects(workspaceId, preserveProject = false) {
  workspaceProjects = [];
  renderProjects();
  if (!preserveProject) {
    elements["project-id"].value = "";
    elements["project-name"].value = "";
  }
  if (!workspaceId) {
    elements["project-hint"].textContent = "先选择工作区，再读取项目。";
    return;
  }
  elements["project-hint"].textContent = "正在读取项目列表…";
  try {
    const result = await bridge().invoke("listWorkspaceProjects", { workspaceId });
    workspaceProjects = Array.isArray(result)
      ? result.filter((project) => Number(project?.projectId) > 0).map((project) => ({
          projectId: Number(project.projectId),
          name: String(project.name || `项目 ${project.projectId}`).trim(),
        }))
      : [];
    renderProjects();
  } catch (error) {
    elements["project-hint"].textContent = `读取项目失败：${errorMessage(error)}`;
  }
}

async function loadState(successMessage = "") {
  clearNotices();
  setBusy(true);
  try {
    credentialState = normalizeCredentialState(await bridge().invoke("credentialState"));
    renderState();
    await loadProjects(preferredWorkspaceId(credentialState), true);
    if (successMessage) setMessage("message", successMessage);
  } catch (error) {
    credentialState = null;
    setMessage("error", errorMessage(error));
    setBusy(false);
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
    payload = profile
      ? buildSwitchPayload(profile)
      : selectedSwitchPayload();
  } catch (error) {
    setMessage("error", errorMessage(error));
    return;
  }
  const project = payload.projectName || `项目 ${payload.projectId}`;
  if (!window.confirm(`将为“${payload.workspaceName}”的“${project}”重新签发 LLM credential，并重启 Codex。继续吗？`)) {
    return;
  }
  setBusy(true);
  try {
    const result = await bridge().invoke("switchCredential", payload);
    credentialState = normalizeCredentialState(result?.state);
    renderState();
    await loadProjects(payload.workspaceId, true);
    const restart = String(result?.restartMessage || "Codex 重启状态未返回。");
    setMessage("message", `已切换到 ${payload.workspaceName} / ${project}。${restart}`);
  } catch (error) {
    setMessage("error", errorMessage(error));
    setBusy(false);
  }
}

elements["refresh-button"].addEventListener("click", () => void loadState("状态已刷新。"));
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
  void loadProjects(workspaceId, active?.workspaceId === workspaceId);
});
elements["project-id"].addEventListener("input", () => {
  const selected = workspaceProjects.find((project) => project.projectId === Number(elements["project-id"].value));
  if (selected) elements["project-name"].value = selected.name;
});
elements["switch-form"].addEventListener("submit", (event) => {
  event.preventDefault();
  void switchCredential();
});

void loadState();
