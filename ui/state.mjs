export const DEFAULT_MODEL = "gpt-5.6-sol";

function positiveInteger(value) {
  const number = Number(value);
  return Number.isInteger(number) && number > 0 ? number : 0;
}

export function normalizeCredentialState(value) {
  const input = value && typeof value === "object" ? value : {};
  const workspaces = Array.isArray(input.workspaces)
    ? input.workspaces
        .map((workspace) => ({
          workspaceId: positiveInteger(workspace?.workspaceId),
          name: String(workspace?.name || "").trim(),
        }))
        .filter((workspace) => workspace.workspaceId > 0)
    : [];
  const profiles = Array.isArray(input.profiles)
    ? input.profiles.map(normalizeProfile).filter(Boolean)
    : [];
  return {
    codexConfigured: input.codexConfigured === true,
    credentialStatus: String(input.credentialStatus || "not_configured"),
    activeProfile: normalizeProfile(input.activeProfile),
    profiles,
    workspaces,
    discoveryWarning: typeof input.discoveryWarning === "string" ? input.discoveryWarning.trim() : "",
  };
}

export function normalizeProfile(value) {
  if (!value || typeof value !== "object") return null;
  const workspaceId = positiveInteger(value.workspaceId);
  const projectId = positiveInteger(value.projectId);
  if (!workspaceId || !projectId) return null;
  return {
    workspaceId,
    workspaceName: String(value.workspaceName || `工作区 ${workspaceId}`).trim(),
    projectId,
    projectName: typeof value.projectName === "string" ? value.projectName.trim() : "",
    model: String(value.model || DEFAULT_MODEL).trim() || DEFAULT_MODEL,
    activatedAtEpochSeconds: Math.max(0, Number(value.activatedAtEpochSeconds) || 0),
  };
}

export function credentialStatusMeta(status) {
  switch (status) {
    case "verified":
      return { label: "已验证", tone: "success" };
    case "invalid":
      return { label: "凭证无效", tone: "danger" };
    case "invalid_context":
      return { label: "归属异常", tone: "danger" };
    case "unverified":
      return { label: "暂未验证", tone: "warning" };
    case "not_configured":
      return { label: "尚未配置", tone: "neutral" };
    default:
      return { label: "状态未知", tone: "neutral" };
  }
}

export function preferredWorkspaceId(state, currentValue = "") {
  const current = positiveInteger(currentValue);
  if (current && state.workspaces.some((workspace) => workspace.workspaceId === current)) return current;
  const active = state.activeProfile?.workspaceId || 0;
  if (active && state.workspaces.some((workspace) => workspace.workspaceId === active)) return active;
  return state.workspaces[0]?.workspaceId || 0;
}

export function buildSwitchPayload(input) {
  const workspaceId = positiveInteger(input.workspaceId);
  const projectId = positiveInteger(input.projectId);
  if (!workspaceId) throw new Error("请选择要切换的工作区。");
  if (!projectId) throw new Error("请输入有效的项目 ID；Codex 调用必须有明确的项目归属。");
  const model = String(input.model || "").trim();
  if (!model) throw new Error("模型不能为空。");
  return {
    workspaceId,
    workspaceName: String(input.workspaceName || `工作区 ${workspaceId}`).trim(),
    projectId,
    projectName: String(input.projectName || "").trim() || null,
    model,
  };
}

export function projectLabel(profile) {
  return profile.projectName || `项目 ${profile.projectId}`;
}

export function formatActivatedAt(epochSeconds) {
  if (!epochSeconds) return "尚未记录切换时间";
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(epochSeconds * 1000));
}
