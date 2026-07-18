use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const METADATA_VERSION: u32 = 1;
const METADATA_FILE: &str = "codex-credentials.json";
const ROUTER_BASE_URL: &str = "https://router.baijimu.com/api/claudecode/v1";
const DEFAULT_MODEL: &str = "gpt-5.6-sol";
const MANAGED_BLOCK_START: &str = "# >>> baijimu managed codex router";
const MANAGED_BLOCK_END: &str = "# <<< baijimu managed codex router";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialProfile {
    pub workspace_id: u64,
    pub workspace_name: String,
    pub project_id: u64,
    pub project_name: Option<String>,
    pub model: String,
    pub activated_at_epoch_seconds: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceOption {
    pub workspace_id: u64,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOption {
    pub project_id: u64,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialManagerState {
    pub codex_configured: bool,
    pub credential_status: String,
    pub active_profile: Option<CredentialProfile>,
    pub profiles: Vec<CredentialProfile>,
    pub workspaces: Vec<WorkspaceOption>,
    pub discovery_warning: Option<String>,
    pub shared_auth_path: String,
    pub codex_auth_path: String,
    pub codex_config_path: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSwitchRequest {
    pub workspace_id: u64,
    #[serde(default)]
    pub workspace_name: String,
    pub project_id: u64,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSwitchResult {
    pub state: CredentialManagerState,
    pub codex_restarted: bool,
    pub restart_message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialMetadata {
    version: u32,
    #[serde(default)]
    profiles: Vec<CredentialProfile>,
    active_workspace_id: Option<u64>,
    active_project_id: Option<u64>,
}

impl Default for CredentialMetadata {
    fn default() -> Self {
        Self {
            version: METADATA_VERSION,
            profiles: Vec::new(),
            active_workspace_id: None,
            active_project_id: None,
        }
    }
}

#[derive(Clone, Debug)]
struct LocalMachineCredential {
    workspace_id: u64,
    token: String,
}

#[derive(Clone, Debug)]
struct SharedCredentialStore {
    base_url: String,
    credentials: Vec<LocalMachineCredential>,
}

pub fn state() -> Result<CredentialManagerState> {
    let store = load_shared_credential_store()?;
    let mut metadata = load_metadata()?;
    let (mut workspaces, discovery_warning) = discover_workspaces(&store);
    if workspaces.is_empty() {
        workspaces = store
            .credentials
            .iter()
            .map(|credential| WorkspaceOption {
                workspace_id: credential.workspace_id,
                name: format!("工作区 {}", credential.workspace_id),
            })
            .collect();
    }
    workspaces.sort_by(|left, right| left.name.cmp(&right.name));
    workspaces.dedup_by_key(|item| item.workspace_id);

    for profile in &mut metadata.profiles {
        if let Some(workspace) = workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == profile.workspace_id)
        {
            profile.workspace_name = workspace.name.clone();
        }
    }

    let auth_path = codex_auth_path();
    let config_path = codex_config_path();
    let current_key = read_codex_api_key(&auth_path)?;
    let managed_config = fs::read_to_string(&config_path)
        .map(|content| content.contains(MANAGED_BLOCK_START))
        .unwrap_or(false);
    let mut credential_status = if current_key.is_some() {
        "unverified".to_string()
    } else {
        "not_configured".to_string()
    };
    let mut active_profile = metadata
        .active_workspace_id
        .zip(metadata.active_project_id)
        .and_then(|(workspace_id, project_id)| {
            metadata
                .profiles
                .iter()
                .find(|profile| {
                    profile.workspace_id == workspace_id && profile.project_id == project_id
                })
                .cloned()
        });

    if let Some(key) = current_key.as_deref() {
        match validate_credential(&store.base_url, key) {
            Ok(Some(validated)) => {
                let workspace_id = validated.get("workspaceId").and_then(Value::as_u64);
                let project_id = validated.get("projectId").and_then(Value::as_u64);
                if let (Some(workspace_id), Some(project_id)) = (workspace_id, project_id) {
                    if active_profile.as_ref().is_none_or(|profile| {
                        profile.workspace_id != workspace_id || profile.project_id != project_id
                    }) {
                        active_profile = Some(CredentialProfile {
                            workspace_id,
                            workspace_name: workspaces
                                .iter()
                                .find(|workspace| workspace.workspace_id == workspace_id)
                                .map(|workspace| workspace.name.clone())
                                .unwrap_or_else(|| format!("工作区 {workspace_id}")),
                            project_id,
                            project_name: None,
                            model: DEFAULT_MODEL.to_string(),
                            activated_at_epoch_seconds: 0,
                        });
                    }
                    credential_status = "verified".to_string();
                } else {
                    credential_status = "invalid_context".to_string();
                }
            }
            Ok(None) => credential_status = "invalid".to_string(),
            Err(_) => credential_status = "unverified".to_string(),
        }
    }

    Ok(CredentialManagerState {
        codex_configured: current_key.is_some() && managed_config,
        credential_status,
        active_profile,
        profiles: metadata.profiles,
        workspaces,
        discovery_warning,
        shared_auth_path: shared_auth_path().display().to_string(),
        codex_auth_path: auth_path.display().to_string(),
        codex_config_path: config_path.display().to_string(),
    })
}

pub fn list_workspace_projects(workspace_id: u64) -> Result<Vec<ProjectOption>> {
    if workspace_id == 0 {
        anyhow::bail!("工作区 ID 必须大于 0");
    }
    let store = load_shared_credential_store()?;
    let token = select_local_machine_token(&store, workspace_id)
        .context("本机没有可用于查询项目的百积木工作区授权，请先重新授权设备")?;
    let response = post_baijimu_json(
        &store.base_url,
        "/lowcode3/api/project/summary/list",
        token,
        json!({"workspaceId": workspace_id, "pageNum": 1, "pageSize": 200}),
    )?;
    let data = unwrap_baijimu_data(&response)?;
    let items = data
        .get("list")
        .or_else(|| data.get("records"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut projects = items
        .iter()
        .filter_map(|item| {
            Some(ProjectOption {
                project_id: item.get("id").and_then(Value::as_u64)?,
                name: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("未命名项目")
                    .trim()
                    .to_string(),
            })
        })
        .collect::<Vec<_>>();
    projects.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(projects)
}

pub fn switch(request: CredentialSwitchRequest) -> Result<CredentialSwitchResult> {
    if request.workspace_id == 0 {
        anyhow::bail!("工作区 ID 必须大于 0");
    }
    if request.project_id == 0 {
        anyhow::bail!("项目 ID 必须大于 0；模型调用会按项目归属和计量");
    }
    let model = normalize_model(request.model.as_deref())?;
    let store = load_shared_credential_store()?;
    let token = select_local_machine_token(&store, request.workspace_id)
        .context("本机没有百积木工作区授权，无法签发 Codex LLM credential，请先重新授权设备")?;
    let response = post_baijimu_json(
        &store.base_url,
        &format!(
            "/llm-credential/partner/v1/workspaces/{}/llm-credentials/create",
            request.workspace_id
        ),
        token,
        json!({"workspaceId": request.workspace_id, "projectId": request.project_id}),
    )?;
    let data = unwrap_baijimu_data(&response)?;
    let credential = ["llmCredential", "credential", "apiKey"]
        .iter()
        .find_map(|field| data.get(*field).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("平台已响应，但没有返回 LLM credential")?
        .to_string();
    let validated = validate_credential(&store.base_url, &credential)?
        .context("新签发的 LLM credential 未通过平台校验，未修改 Codex 配置")?;
    let validated_workspace_id = validated
        .get("workspaceId")
        .and_then(Value::as_u64)
        .context("凭证校验结果缺少 workspaceId，未修改 Codex 配置")?;
    let validated_project_id = validated.get("projectId").and_then(Value::as_u64);
    if validated_workspace_id != request.workspace_id
        || validated_project_id != Some(request.project_id)
    {
        anyhow::bail!(
            "凭证归属校验不一致：期望工作区 {} / 项目 {}，实际工作区 {} / 项目 {}，未修改 Codex 配置",
            request.workspace_id,
            request.project_id,
            validated_workspace_id,
            validated_project_id
                .map(|value| value.to_string())
                .unwrap_or_else(|| "未绑定".to_string())
        );
    }

    let mut metadata = load_metadata()?;
    let profile = CredentialProfile {
        workspace_id: request.workspace_id,
        workspace_name: if request.workspace_name.trim().is_empty() {
            format!("工作区 {}", request.workspace_id)
        } else {
            request.workspace_name.trim().to_string()
        },
        project_id: request.project_id,
        project_name: request
            .project_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        model,
        activated_at_epoch_seconds: now_epoch_seconds(),
    };
    metadata.profiles.retain(|candidate| {
        candidate.workspace_id != request.workspace_id || candidate.project_id != request.project_id
    });
    metadata.profiles.push(profile);
    metadata.profiles.sort_by(|left, right| {
        left.workspace_name
            .cmp(&right.workspace_name)
            .then(left.project_id.cmp(&right.project_id))
    });
    metadata.active_workspace_id = Some(request.workspace_id);
    metadata.active_project_id = Some(request.project_id);
    apply_switch(&credential, &metadata)?;
    drop(credential);

    let (codex_restarted, restart_message) = restart_codex_desktop_app();
    let state = state().context("Codex 已切换，但重新读取状态失败")?;
    if state.active_profile.as_ref().is_none_or(|active| {
        active.workspace_id != request.workspace_id || active.project_id != request.project_id
    }) {
        anyhow::bail!("Codex 配置已写入，但生效回查与目标工作区不一致，请停止使用并重新切换");
    }
    Ok(CredentialSwitchResult {
        state,
        codex_restarted,
        restart_message,
    })
}

fn load_shared_credential_store() -> Result<SharedCredentialStore> {
    let path = shared_auth_path();
    let content = fs::read_to_string(&path)
        .with_context(|| format!("读取百积木本机授权失败: {}", path.display()))?;
    let document: Value = serde_json::from_str(&content)
        .with_context(|| format!("解析百积木本机授权失败: {}", path.display()))?;
    let current_environment = document
        .get("currentEnvironment")
        .and_then(Value::as_str)
        .unwrap_or("prod");
    let configured_base_url = document
        .get("environments")
        .and_then(|value| value.get(current_environment))
        .and_then(|value| value.get("baseUrl"))
        .and_then(Value::as_str)
        .unwrap_or("https://www.baijimu.com");
    let credentials = document
        .get("machineCredentials")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let workspace_id = value.get("workspaceId").and_then(Value::as_u64)?;
            let token = value.get("token").and_then(Value::as_str)?.trim();
            (!token.is_empty()).then(|| LocalMachineCredential {
                workspace_id,
                token: token.to_string(),
            })
        })
        .collect::<Vec<_>>();
    if credentials.is_empty() {
        anyhow::bail!("本机还没有工作区授权，请先在百积木中完成设备授权");
    }
    Ok(SharedCredentialStore {
        base_url: normalize_baijimu_root_url(configured_base_url),
        credentials,
    })
}

fn select_local_machine_token(store: &SharedCredentialStore, workspace_id: u64) -> Option<&str> {
    store
        .credentials
        .iter()
        .find(|credential| credential.workspace_id == workspace_id)
        .or_else(|| store.credentials.first())
        .map(|credential| credential.token.as_str())
}

fn discover_workspaces(store: &SharedCredentialStore) -> (Vec<WorkspaceOption>, Option<String>) {
    let Some(token) = store.credentials.first().map(|item| item.token.as_str()) else {
        return (Vec::new(), Some("本机没有工作区授权".to_string()));
    };
    let result = post_baijimu_json(
        &store.base_url,
        "/lowcode3/partner/v1/workspaces/list",
        token,
        json!({"pageNum": 1, "pageSize": 200}),
    )
    .and_then(|response| {
        let data = unwrap_baijimu_data(&response)?;
        Ok(data
            .get("list")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                Some(WorkspaceOption {
                    workspace_id: item.get("id").and_then(Value::as_u64)?,
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("未命名工作区")
                        .trim()
                        .to_string(),
                })
            })
            .collect::<Vec<_>>())
    });
    match result {
        Ok(workspaces) => (workspaces, None),
        Err(error) => (Vec::new(), Some(format!("暂时无法读取工作区名称：{error}"))),
    }
}

fn post_baijimu_json(base_url: &str, path: &str, token: &str, body: Value) -> Result<Value> {
    let response = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(45))
        .build()
        .context("创建平台请求失败")?
        .post(format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        ))
        .bearer_auth(token)
        .json(&body)
        .send()
        .context("请求百积木平台失败")?;
    let status = response.status();
    let payload = response.text().context("读取百积木平台响应失败")?;
    if !status.is_success() {
        anyhow::bail!("百积木平台返回 HTTP {status}: {}", compact_body(&payload));
    }
    serde_json::from_str(&payload).context("百积木平台返回了无效 JSON")
}

fn validate_credential(base_url: &str, credential: &str) -> Result<Option<Value>> {
    let response = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(45))
        .build()
        .context("创建凭证校验请求失败")?
        .post(format!(
            "{}/llm-credential/validateCredential",
            base_url.trim_end_matches('/')
        ))
        .bearer_auth(credential)
        .json(&json!({"key": credential}))
        .send()
        .context("请求凭证校验服务失败")?;
    let status = response.status();
    if matches!(status.as_u16(), 401 | 403) {
        return Ok(None);
    }
    let payload = response.text().context("读取凭证校验响应失败")?;
    if !status.is_success() {
        anyhow::bail!("凭证校验服务返回 HTTP {status}: {}", compact_body(&payload));
    }
    let response: Value = serde_json::from_str(&payload).context("凭证校验服务返回了无效 JSON")?;
    let data = unwrap_baijimu_data(&response)?;
    let valid = data.get("valid").and_then(Value::as_bool).unwrap_or(false);
    let allowed = data
        .get("allowed")
        .and_then(Value::as_bool)
        .unwrap_or(valid);
    Ok((valid && allowed).then(|| data.clone()))
}

fn unwrap_baijimu_data(response: &Value) -> Result<&Value> {
    if let Some(error_code) = response
        .get("errorCode")
        .or_else(|| response.get("error_code"))
        .and_then(Value::as_str)
    {
        if error_code != "0" {
            let message = response
                .get("value")
                .or_else(|| response.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("平台操作失败");
            anyhow::bail!("{message}（{error_code}）");
        }
        return Ok(response.get("data").unwrap_or(&Value::Null));
    }
    Ok(response.get("data").unwrap_or(response))
}

fn compact_body(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(400).collect()
}

fn normalize_model(model: Option<&str>) -> Result<String> {
    let model = model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_MODEL);
    if model
        .chars()
        .any(|character| !(character.is_ascii_alphanumeric() || "._-".contains(character)))
    {
        anyhow::bail!("模型名称只能包含字母、数字、点、下划线和短横线");
    }
    Ok(model.to_string())
}

fn load_metadata() -> Result<CredentialMetadata> {
    let path = metadata_path();
    if !path.exists() {
        let legacy_path = legacy_metadata_path();
        if !legacy_path.exists() {
            return Ok(CredentialMetadata::default());
        }
        let content = fs::read_to_string(&legacy_path)
            .with_context(|| format!("读取旧版 Codex 凭证元数据失败: {}", legacy_path.display()))?;
        let mut metadata: CredentialMetadata = serde_json::from_str(&content)
            .with_context(|| format!("解析旧版 Codex 凭证元数据失败: {}", legacy_path.display()))?;
        metadata.version = METADATA_VERSION;
        atomic_write_private(&path, &serde_json::to_vec_pretty(&metadata)?)?;
        verify_private_file(&path)?;
        fs::remove_file(&legacy_path)
            .with_context(|| format!("清理旧版 Codex 凭证元数据失败: {}", legacy_path.display()))?;
        return Ok(metadata);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("读取 Codex 凭证元数据失败: {}", path.display()))?;
    let mut metadata: CredentialMetadata = serde_json::from_str(&content)
        .with_context(|| format!("解析 Codex 凭证元数据失败: {}", path.display()))?;
    metadata.version = METADATA_VERSION;
    Ok(metadata)
}

fn read_codex_api_key(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(
        &fs::read_to_string(path)
            .with_context(|| format!("读取 Codex 认证文件失败: {}", path.display()))?,
    )
    .with_context(|| format!("解析 Codex 认证文件失败: {}", path.display()))?;
    Ok(value
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned))
}

fn apply_switch(credential: &str, metadata: &CredentialMetadata) -> Result<()> {
    let auth_path = codex_auth_path();
    let config_path = codex_config_path();
    let metadata_path = metadata_path();
    let old_auth = fs::read(&auth_path).ok();
    let old_config = fs::read(&config_path).ok();
    let old_metadata = fs::read(&metadata_path).ok();
    let mut auth_document = old_auth
        .as_deref()
        .and_then(|content| serde_json::from_slice::<Value>(content).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    auth_document["OPENAI_API_KEY"] = Value::String(credential.to_string());
    auth_document["auth_mode"] = Value::String("apikey".to_string());
    let active_profile = metadata
        .active_workspace_id
        .zip(metadata.active_project_id)
        .and_then(|(workspace_id, project_id)| {
            metadata.profiles.iter().find(|profile| {
                profile.workspace_id == workspace_id && profile.project_id == project_id
            })
        })
        .context("Codex 凭证元数据缺少当前配置")?;
    let existing_config = old_config
        .as_deref()
        .map(String::from_utf8_lossy)
        .unwrap_or_default();
    let config_content = render_managed_config(&existing_config, &active_profile.model);
    toml::from_str::<toml::Value>(&config_content)
        .context("生成的 Codex config.toml 无法通过 TOML 校验")?;
    let result = (|| -> Result<()> {
        atomic_write_private(&auth_path, &serde_json::to_vec_pretty(&auth_document)?)?;
        atomic_write_private(&config_path, config_content.as_bytes())?;
        atomic_write_private(&metadata_path, &serde_json::to_vec_pretty(metadata)?)?;
        verify_private_file(&auth_path)?;
        verify_private_file(&config_path)?;
        verify_private_file(&metadata_path)?;
        if read_codex_api_key(&auth_path)?.as_deref() != Some(credential) {
            anyhow::bail!("Codex 认证文件写入后回读不一致");
        }
        Ok(())
    })();
    if let Err(error) = result {
        restore_optional_file(&auth_path, old_auth.as_deref());
        restore_optional_file(&config_path, old_config.as_deref());
        restore_optional_file(&metadata_path, old_metadata.as_deref());
        return Err(error.context("切换失败，已恢复切换前的 Codex 配置"));
    }
    Ok(())
}

fn render_managed_config(existing: &str, model: &str) -> String {
    let mut preserved = Vec::new();
    let mut in_managed_block = false;
    let mut in_router_table = false;
    let mut seen_table = false;
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed == MANAGED_BLOCK_START {
            in_managed_block = true;
            continue;
        }
        if in_managed_block {
            if trimmed == MANAGED_BLOCK_END {
                in_managed_block = false;
            }
            continue;
        }
        if trimmed == "[model_providers.baijimu-router]" {
            in_router_table = true;
            seen_table = true;
            continue;
        }
        if in_router_table {
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                in_router_table = false;
                preserved.push(line.to_string());
            }
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            seen_table = true;
        }
        let managed_root_key = !seen_table
            && [
                "model_provider",
                "model",
                "sandbox_mode",
                "approval_policy",
                "cli_auth_credentials_store",
                "forced_login_method",
            ]
            .iter()
            .any(|key| {
                trimmed
                    .strip_prefix(key)
                    .is_some_and(|rest| rest.trim_start().starts_with('='))
            });
        if !managed_root_key {
            preserved.push(line.to_string());
        }
    }
    while preserved.first().is_some_and(|line| line.trim().is_empty()) {
        preserved.remove(0);
    }
    while preserved.last().is_some_and(|line| line.trim().is_empty()) {
        preserved.pop();
    }
    let first_table = preserved
        .iter()
        .position(|line| {
            let trimmed = line.trim();
            trimmed.starts_with('[') && trimmed.ends_with(']')
        })
        .unwrap_or(preserved.len());
    let (root, tables) = preserved.split_at(first_table);
    let escaped_model = model.replace('\\', "\\\\").replace('"', "\\\"");
    let mut output = format!(
        "{MANAGED_BLOCK_START}\n\
model_provider = \"baijimu-router\"\n\
model = \"{escaped_model}\"\n\
sandbox_mode = \"danger-full-access\"\n\
approval_policy = \"on-request\"\n\
cli_auth_credentials_store = \"file\"\n\
forced_login_method = \"api\"\n\
{MANAGED_BLOCK_END}\n"
    );
    if !root.is_empty() {
        output.push('\n');
        output.push_str(&root.join("\n"));
        output.push('\n');
    }
    output.push_str(&format!(
        "\n[model_providers.baijimu-router]\n\
name = \"baijimu-router\"\n\
base_url = \"{ROUTER_BASE_URL}\"\n\
wire_api = \"responses\"\n\
requires_openai_auth = true\n"
    ));
    if !tables.is_empty() {
        output.push('\n');
        output.push_str(&tables.join("\n"));
        output.push('\n');
    }
    output
}

fn restart_codex_desktop_app() -> (bool, String) {
    #[cfg(target_os = "macos")]
    {
        let app_path = ["/Applications/Codex.app", "/Applications/ChatGPT.app"]
            .iter()
            .map(PathBuf::from)
            .find(|path| path.exists());
        let Some(app_path) = app_path else {
            return (
                false,
                "未找到 Codex/ChatGPT 桌面应用；终端配置已经生效".to_string(),
            );
        };
        let _ = Command::new("pkill")
            .args(["-f", &format!("{}/Contents", app_path.display())])
            .status();
        std::thread::sleep(Duration::from_millis(900));
        return match Command::new("open").arg(&app_path).status() {
            Ok(status) if status.success() => (true, "Codex 已按新工作区重新启动".to_string()),
            Ok(status) => (
                false,
                format!("Codex 配置已切换，但重新启动返回状态 {status}"),
            ),
            Err(error) => (
                false,
                format!("Codex 配置已切换，但自动重新启动失败: {error}"),
            ),
        };
    }
    #[cfg(target_os = "windows")]
    {
        let script = r#"Get-Process Codex,ChatGPT -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue; Start-Sleep -Milliseconds 900; $app = Get-StartApps | Where-Object { $_.Name -match 'Codex|ChatGPT' } | Select-Object -First 1; if (-not $app) { exit 3 }; Start-Process (\"shell:AppsFolder\\\" + $app.AppID)"#;
        return match Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ])
            .status()
        {
            Ok(status) if status.success() => (true, "Codex 已按新工作区重新启动".to_string()),
            Ok(status) => (
                false,
                format!("Codex 配置已切换，但没有找到可重新启动的 Codex 应用（{status}）"),
            ),
            Err(error) => (
                false,
                format!("Codex 配置已切换，但自动重新启动失败: {error}"),
            ),
        };
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        (
            false,
            "Codex 终端配置已切换；当前系统不支持自动重启桌面应用".to_string(),
        )
    }
}

fn normalize_baijimu_root_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    trimmed
        .strip_suffix("/lowcode3")
        .unwrap_or(trimmed)
        .to_string()
}

fn shared_auth_path() -> PathBuf {
    if let Some(config_home) = std::env::var_os("BAIJIMU_CONFIG_HOME") {
        return PathBuf::from(config_home).join("baijimu").join("auth.json");
    }
    home_dir().join(".config").join("baijimu").join("auth.json")
}

fn codex_home_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex"))
}

fn codex_auth_path() -> PathBuf {
    codex_home_dir().join("auth.json")
}

fn codex_config_path() -> PathBuf {
    codex_home_dir().join("config.toml")
}

fn metadata_path() -> PathBuf {
    connector_data_dir().join(METADATA_FILE)
}

fn legacy_metadata_path() -> PathBuf {
    shared_auth_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(METADATA_FILE)
}

fn connector_data_dir() -> PathBuf {
    std::env::var_os("BAIJIMU_CONNECTOR_DATA_DIR")
        .or_else(|| std::env::var_os("CODEX_CONNECTOR_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".baijimu-connector-codex"))
}

fn home_dir() -> PathBuf {
    #[cfg(windows)]
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(profile);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn atomic_write_private(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
        set_private_directory(parent)?;
    }
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temp_path = path.with_extension(format!("tmp-{}-{unique}", std::process::id()));
    fs::write(&temp_path, content)
        .with_context(|| format!("写入临时文件失败: {}", temp_path.display()))?;
    set_private_file(&temp_path)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&temp_path, path).with_context(|| format!("替换文件失败: {}", path.display()))?;
    set_private_file(path)?;
    Ok(())
}

fn verify_private_file(path: &Path) -> Result<()> {
    let metadata =
        fs::metadata(path).with_context(|| format!("回读文件失败: {}", path.display()))?;
    if !metadata.is_file() || metadata.len() == 0 {
        anyhow::bail!("文件为空或不是普通文件: {}", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!("文件权限不是 600: {}", path.display());
        }
    }
    Ok(())
}

fn restore_optional_file(path: &Path, content: Option<&[u8]>) {
    match content {
        Some(content) => {
            let _ = atomic_write_private(path, content);
        }
        None => {
            let _ = fs::remove_file(path);
        }
    }
}

fn set_private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    struct EnvironmentRestore {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvironmentRestore {
        fn set(key: &'static str, value: &Path) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvironmentRestore {
        fn drop(&mut self) {
            match self.previous.as_ref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn managed_config_is_idempotent_and_preserves_other_tables() {
        let existing = "model = \"old\"\n[projects.\"/tmp/demo\"]\ntrust_level = \"trusted\"\n";
        let first = render_managed_config(existing, DEFAULT_MODEL);
        let second = render_managed_config(&first, DEFAULT_MODEL);
        assert_eq!(first, second);
        assert_eq!(second.matches(MANAGED_BLOCK_START).count(), 1);
        assert!(second.contains("[projects.\"/tmp/demo\"]"));
        assert!(toml::from_str::<toml::Value>(&second).is_ok());
    }

    #[test]
    fn model_name_rejects_toml_injection() {
        assert!(normalize_model(Some("gpt-5.6-sol")).is_ok());
        assert!(normalize_model(Some("bad\"\nmodel = \"other")).is_err());
    }

    #[test]
    fn legacy_metadata_is_migrated_into_connector_data_directory() {
        let root = std::env::temp_dir().join(format!(
            "baijimu-codex-metadata-test-{}-{}",
            std::process::id(),
            now_epoch_seconds()
        ));
        let config_home = root.join("config");
        let data_dir = root.join("connector-data");
        fs::create_dir_all(config_home.join("baijimu")).unwrap();
        let _config = EnvironmentRestore::set("BAIJIMU_CONFIG_HOME", &config_home);
        let _data = EnvironmentRestore::set("BAIJIMU_CONNECTOR_DATA_DIR", &data_dir);
        let legacy_path = legacy_metadata_path();
        fs::write(
            &legacy_path,
            serde_json::to_vec_pretty(&CredentialMetadata {
                version: METADATA_VERSION,
                profiles: vec![CredentialProfile {
                    workspace_id: 12,
                    workspace_name: "测试工作区".to_string(),
                    project_id: 34,
                    project_name: Some("测试项目".to_string()),
                    model: DEFAULT_MODEL.to_string(),
                    activated_at_epoch_seconds: 56,
                }],
                active_workspace_id: Some(12),
                active_project_id: Some(34),
            })
            .unwrap(),
        )
        .unwrap();

        let metadata = load_metadata().unwrap();
        assert_eq!(metadata.active_workspace_id, Some(12));
        assert_eq!(metadata.active_project_id, Some(34));
        assert!(metadata_path().exists());
        assert!(!legacy_path.exists());
        verify_private_file(&metadata_path()).unwrap();

        fs::remove_dir_all(root).unwrap();
    }
}
