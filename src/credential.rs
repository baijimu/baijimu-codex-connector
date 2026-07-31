use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const METADATA_VERSION: u32 = 1;
const METADATA_FILE: &str = "codex-credentials.json";
const DEFAULT_MODEL: &str = "gpt-5.6-sol";
const MANAGED_BLOCK_START: &str = "# >>> baijimu managed codex router";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialProfile {
    pub workspace_id: u64,
    pub workspace_name: String,
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
pub struct CredentialManagerState {
    pub current_workspace_id: Option<u64>,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialMetadata {
    version: u32,
    #[serde(default)]
    profiles: Vec<CredentialProfile>,
    active_workspace_id: Option<u64>,
}

impl Default for CredentialMetadata {
    fn default() -> Self {
        Self {
            version: METADATA_VERSION,
            profiles: Vec::new(),
            active_workspace_id: None,
        }
    }
}

#[derive(Clone, Debug)]
struct LocalMachineCredential {
    workspace_ids: Vec<u64>,
    token: String,
    issued_at_epoch_seconds: u64,
}

#[derive(Clone, Debug)]
struct SharedCredentialStore {
    base_url: String,
    current_workspace_id: Option<u64>,
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
            .flat_map(|credential| credential.workspace_ids.iter().copied())
            .map(|workspace_id| WorkspaceOption {
                workspace_id,
                name: format!("工作区 {workspace_id}"),
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
    let mut active_profile = metadata.active_workspace_id.and_then(|workspace_id| {
        metadata
            .profiles
            .iter()
            .find(|profile| profile.workspace_id == workspace_id)
            .cloned()
    });

    if let Some(key) = current_key.as_deref() {
        match validate_credential(&store.base_url, key) {
            Ok(Some(validated)) => {
                let workspace_id = validated.get("workspaceId").and_then(Value::as_u64);
                if let Some(workspace_id) = workspace_id {
                    if active_profile
                        .as_ref()
                        .is_none_or(|profile| profile.workspace_id != workspace_id)
                    {
                        active_profile = Some(CredentialProfile {
                            workspace_id,
                            workspace_name: workspaces
                                .iter()
                                .find(|workspace| workspace.workspace_id == workspace_id)
                                .map(|workspace| workspace.name.clone())
                                .unwrap_or_else(|| format!("工作区 {workspace_id}")),
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
        current_workspace_id: store.current_workspace_id,
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
        .get("credentials")
        .or_else(|| document.get("machineCredentials"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let mut workspace_ids = value
                .get("workspaceIds")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .filter(|workspace_id| *workspace_id > 0)
                .collect::<Vec<_>>();
            if let Some(workspace_id) = value
                .get("workspaceId")
                .and_then(Value::as_u64)
                .filter(|workspace_id| *workspace_id > 0)
            {
                workspace_ids.push(workspace_id);
            }
            workspace_ids.sort_unstable();
            workspace_ids.dedup();
            let token = value.get("token").and_then(Value::as_str)?.trim();
            (!token.is_empty() && !workspace_ids.is_empty()).then(|| LocalMachineCredential {
                workspace_ids,
                token: token.to_string(),
                issued_at_epoch_seconds: value
                    .get("issuedAtEpochSeconds")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();
    if credentials.is_empty() {
        anyhow::bail!("本机还没有工作区授权，请先在百积木中完成设备授权");
    }
    Ok(SharedCredentialStore {
        base_url: normalize_baijimu_root_url(configured_base_url),
        current_workspace_id: document.get("currentWorkspaceId").and_then(Value::as_u64),
        credentials,
    })
}

fn select_local_machine_token(store: &SharedCredentialStore, workspace_id: u64) -> Option<&str> {
    store
        .credentials
        .iter()
        .filter(|credential| credential.workspace_ids.contains(&workspace_id))
        .max_by_key(|credential| credential.issued_at_epoch_seconds)
        .map(|credential| credential.token.as_str())
}

pub fn issue_workspace_credential(workspace_id: u64) -> Result<String> {
    if workspace_id == 0 {
        anyhow::bail!("工作区 ID 必须大于 0");
    }
    let store = load_shared_credential_store()?;
    if store
        .current_workspace_id
        .is_some_and(|current| current != workspace_id)
    {
        anyhow::bail!(
            "客户端当前工作区与本机授权不一致：客户端为 {workspace_id}，本机授权当前工作区为 {}",
            store.current_workspace_id.unwrap_or_default()
        );
    }
    let token = select_local_machine_token(&store, workspace_id)
        .context("本机授权不包含客户端当前工作区，无法签发 Codex LLM credential")?;
    let response = post_baijimu_json(
        &store.base_url,
        &format!("/llm-credential/partner/v1/workspaces/{workspace_id}/llm-credentials/create"),
        token,
        json!({"workspaceId": workspace_id, "projectId": null}),
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
        .context("新签发的 LLM credential 未通过平台校验")?;
    let validated_workspace_id = validated
        .get("workspaceId")
        .and_then(Value::as_u64)
        .context("凭证校验结果缺少 workspaceId")?;
    if validated_workspace_id != workspace_id {
        anyhow::bail!(
            "凭证归属校验不一致：期望工作区 {workspace_id}，实际工作区 {validated_workspace_id}"
        );
    }
    if validated
        .get("projectId")
        .is_some_and(|value| !value.is_null())
    {
        anyhow::bail!("平台返回了项目级凭证，连接器要求工作区级凭证");
    }
    Ok(credential)
}

pub fn current_workspace_id() -> Result<u64> {
    let store = load_shared_credential_store()?;
    if let Some(workspace_id) = store.current_workspace_id.filter(|value| *value > 0) {
        return Ok(workspace_id);
    }
    let mut workspace_ids = store
        .credentials
        .iter()
        .flat_map(|credential| credential.workspace_ids.iter().copied())
        .collect::<Vec<_>>();
    workspace_ids.sort_unstable();
    workspace_ids.dedup();
    match workspace_ids.as_slice() {
        [workspace_id] => Ok(*workspace_id),
        [] => anyhow::bail!("本机授权不包含工作区"),
        _ => anyhow::bail!("本机授权包含多个工作区，但没有设置当前工作区"),
    }
}

pub fn record_workspace_setup(workspace_id: u64) -> Result<()> {
    let store = load_shared_credential_store()?;
    let (workspaces, _) = discover_workspaces(&store);
    let workspace_name = workspaces
        .into_iter()
        .find(|workspace| workspace.workspace_id == workspace_id)
        .map(|workspace| workspace.name)
        .unwrap_or_else(|| format!("工作区 {workspace_id}"));
    let mut metadata = load_metadata()?;
    metadata
        .profiles
        .retain(|profile| profile.workspace_id != workspace_id);
    metadata.profiles.push(CredentialProfile {
        workspace_id,
        workspace_name,
        model: DEFAULT_MODEL.to_string(),
        activated_at_epoch_seconds: now_epoch_seconds(),
    });
    metadata
        .profiles
        .sort_by(|left, right| left.workspace_name.cmp(&right.workspace_name));
    metadata.active_workspace_id = Some(workspace_id);
    atomic_write_private(&metadata_path(), &serde_json::to_vec_pretty(&metadata)?)?;
    verify_private_file(&metadata_path())
}

pub fn codex_ready_for_workspace(workspace_id: u64) -> bool {
    state().is_ok_and(|state| {
        state.codex_configured
            && state.credential_status == "verified"
            && state
                .active_profile
                .is_some_and(|profile| profile.workspace_id == workspace_id)
    })
}

fn discover_workspaces(store: &SharedCredentialStore) -> (Vec<WorkspaceOption>, Option<String>) {
    let token = store
        .current_workspace_id
        .and_then(|workspace_id| select_local_machine_token(store, workspace_id))
        .or_else(|| store.credentials.first().map(|item| item.token.as_str()));
    let Some(token) = token else {
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

fn normalize_baijimu_root_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    let root = trimmed.strip_suffix("/lowcode3").unwrap_or(trimmed);
    match root {
        "https://www.baijimu.com" | "https://baijimu.com" => "https://api.baijimu.com".to_string(),
        _ => root.to_string(),
    }
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

    static ENVIRONMENT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    fn legacy_metadata_is_migrated_into_connector_data_directory() {
        let _guard = ENVIRONMENT_LOCK.lock().unwrap();
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
                    model: DEFAULT_MODEL.to_string(),
                    activated_at_epoch_seconds: 56,
                }],
                active_workspace_id: Some(12),
            })
            .unwrap(),
        )
        .unwrap();

        let metadata = load_metadata().unwrap();
        assert_eq!(metadata.active_workspace_id, Some(12));
        assert!(metadata_path().exists());
        assert!(!legacy_path.exists());
        verify_private_file(&metadata_path()).unwrap();

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unified_auth_store_selects_only_the_requested_workspace() {
        let _guard = ENVIRONMENT_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!(
            "baijimu-codex-auth-test-{}-{}",
            std::process::id(),
            now_epoch_seconds()
        ));
        let config_home = root.join("config");
        fs::create_dir_all(config_home.join("baijimu")).unwrap();
        let _config = EnvironmentRestore::set("BAIJIMU_CONFIG_HOME", &config_home);
        fs::write(
            shared_auth_path(),
            serde_json::to_vec_pretty(&json!({
                "schemaVersion": 2,
                "currentEnvironment": "prod",
                "currentWorkspaceId": 1390,
                "environments": {"prod": {"baseUrl": "https://api.baijimu.com"}},
                "credentials": [
                    {"workspaceIds": [1200], "token": "lc_pat_wrong", "issuedAtEpochSeconds": 20},
                    {"workspaceIds": [1390, 1400], "token": "lc_pat_old", "issuedAtEpochSeconds": 10},
                    {"workspaceIds": [1390], "token": "lc_pat_current", "issuedAtEpochSeconds": 30}
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let store = load_shared_credential_store().unwrap();
        assert_eq!(store.current_workspace_id, Some(1390));
        assert_eq!(current_workspace_id().unwrap(), 1390);
        assert_eq!(
            select_local_machine_token(&store, 1390),
            Some("lc_pat_current")
        );
        assert_eq!(
            select_local_machine_token(&store, 1200),
            Some("lc_pat_wrong")
        );
        assert_eq!(select_local_machine_token(&store, 9999), None);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn production_website_auth_endpoint_maps_to_api_origin() {
        assert_eq!(
            normalize_baijimu_root_url("https://www.baijimu.com/lowcode3/"),
            "https://api.baijimu.com"
        );
        assert_eq!(
            normalize_baijimu_root_url("https://api.baijimu.com/lowcode3"),
            "https://api.baijimu.com"
        );
    }
}
