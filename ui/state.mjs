export const DEFAULT_MODEL = "gpt-5.6-sol";

function positiveInteger(value) {
  const number = Number(value);
  return Number.isInteger(number) && number > 0 ? number : null;
}

export function normalizeCredentialState(value) {
  const input = value && typeof value === "object" ? value : {};
  const workspaces = Array.isArray(input.workspaces)
    ? input.workspaces
        .map((workspace) => ({
          workspaceId: positiveInteger(workspace?.workspaceId),
          name: String(workspace?.name || "").trim(),
          authorized: workspace?.authorized === true,
          configured: workspace?.configured === true,
          userIds: Array.isArray(workspace?.userIds) ? workspace.userIds.map(positiveInteger).filter(Boolean) : [],
        }))
        .filter((workspace) => workspace.workspaceId)
    : [];
  const profiles = Array.isArray(input.profiles)
    ? input.profiles.map(normalizeProfile).filter(Boolean)
    : [];
  return {
    activeMode: input.activeMode === "baijimu" ? "baijimu" : "chatgpt",
    currentWorkspaceId: positiveInteger(input.currentWorkspaceId),
    activeWorkspaceId: positiveInteger(input.activeWorkspaceId),
    codexConfigured: input.codexConfigured === true,
    credentialStatus: String(input.credentialStatus || "not_configured"),
    activeProfile: normalizeProfile(input.activeProfile),
    profiles,
    workspaces,
    chatgpt: {
      configured: input.chatgpt?.configured === true,
      authMode: String(input.chatgpt?.authMode || ""),
      accountId: String(input.chatgpt?.accountId || ""),
      codexHome: String(input.chatgpt?.codexHome || ""),
    },
    originalCodexHome: String(input.originalCodexHome || input.chatgpt?.codexHome || ""),
    originalCodexHomeState: {
      captured: input.originalCodexHomeState?.captured === true,
      wasSet: typeof input.originalCodexHomeState?.value === "string",
      value: typeof input.originalCodexHomeState?.value === "string" ? input.originalCodexHomeState.value : "",
      captureSource: String(input.originalCodexHomeState?.captureSource || ""),
    },
    activeCodexHome: String(input.activeCodexHome || ""),
    userCodexHome: typeof input.userCodexHome === "string" ? input.userCodexHome : "",
    userCodexHomeSynchronized: input.userCodexHomeSynchronized === true,
    desktopEnvironmentManaged: input.desktopEnvironmentManaged === true,
    discoveryWarning: typeof input.discoveryWarning === "string" ? input.discoveryWarning.trim() : "",
  };
}

export function normalizeProfile(value) {
  if (!value || typeof value !== "object") return null;
  const workspaceId = positiveInteger(value.workspaceId);
  if (!workspaceId) return null;
  return {
    profileId: String(value.profileId || ""),
    environment: String(value.environment || "prod"),
    userId: positiveInteger(value.userId),
    clientId: String(value.clientId || ""),
    workspaceId,
    workspaceName: String(value.workspaceName || `工作区 ${workspaceId}`).trim(),
    model: String(value.model || DEFAULT_MODEL).trim() || DEFAULT_MODEL,
    activatedAtEpochSeconds: Math.max(0, Number(value.activatedAtEpochSeconds) || 0),
    codexHome: String(value.codexHome || ""),
    credentialStatus: String(value.credentialStatus || ""),
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

export function normalizeSetupProgress(value) {
  const setupStatus = String(value?.status || "pending");
  const installer = value?.installerStatus && typeof value.installerStatus === "object"
    ? value.installerStatus
    : {};
  const steps = (Array.isArray(installer.steps) ? installer.steps : []).map((step, index) => ({
    index: Math.max(1, Number(step?.index) || index + 1),
    name: String(step?.name || `步骤 ${index + 1}`),
    state: String(step?.state || "pending"),
    detail: String(step?.detail || ""),
    downloadedBytes: step?.downloadedBytes != null && Number.isFinite(Number(step.downloadedBytes))
      ? Number(step.downloadedBytes)
      : null,
    totalBytes: step?.totalBytes != null && Number.isFinite(Number(step.totalBytes))
      ? Number(step.totalBytes)
      : null,
  }));
  const finishedStates = new Set(["completed", "skipped"]);
  const finished = steps.filter((step) => finishedStates.has(step.state)).length;
  const current = steps.find((step) => step.state === "running");
  const downloadFraction = current?.totalBytes > 0 && current?.downloadedBytes >= 0
    ? Math.min(1, current.downloadedBytes / current.totalBytes)
    : 0;
  const calculated = steps.length > 0
    ? Math.round(((finished + downloadFraction) / steps.length) * 100)
    : 0;
  const percent = setupStatus === "succeeded" ? 100 : Math.max(0, Math.min(99, calculated));
  return {
    status: setupStatus,
    percent,
    currentStep: Math.max(0, Number(installer.currentStep) || current?.index || 0),
    startedAt: String(installer.startedAt || ""),
    updatedAt: String(installer.updatedAt || ""),
    steps,
  };
}

export function shouldShowSetupProgress(value) {
  const progress = normalizeSetupProgress(value);
  return progress.status !== "succeeded" && progress.steps.length > 0;
}
