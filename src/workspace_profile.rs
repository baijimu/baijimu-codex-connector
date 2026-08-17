use crate::{baijimu_cli, process_runtime::connector_home};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use toml_edit::{value, DocumentMut, Item, Table};

const PROFILE_SCHEMA_VERSION: u32 = 1;
const PROFILE_CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceProfileConfig {
    schema_version: u32,
    default_model: String,
    router_provider: String,
    router_base_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceProfileMetadata {
    schema_version: u32,
    environment: String,
    workspace_id: u64,
    workspace_name: String,
    created_at_epoch_seconds: u64,
    updated_at_epoch_seconds: u64,
}

pub(crate) struct PreparedProfile {
    pub(crate) environment: String,
    pub(crate) codex_home: PathBuf,
    pub(crate) state_dir: PathBuf,
}

pub(crate) fn ensure(workspace_id: u64) -> Result<PreparedProfile> {
    if workspace_id == 0 {
        bail!("workspaceId 必须是正整数");
    }
    let auth = baijimu_cli::auth_status().context("读取 baijimu CLI 授权状态失败")?;
    if !auth.authenticated || !auth.workspace_ids.contains(&workspace_id) {
        bail!("当前设备授权不包含工作区 {workspace_id}");
    }
    let workspace =
        baijimu_cli::get_workspace(workspace_id).context("无法确认远程调用所属工作区")?;
    let profile_config = workspace_profile_config()?;
    let home = profile_home(&auth.base_url, workspace_id);
    let metadata_path = home.join("profile.json");
    let auth_path = home.join("auth.json");
    let config_path = home.join("config.toml");
    if metadata_matches(&metadata_path, &auth.base_url, workspace_id)
        && auth_file_ready(&auth_path)
        && config_file_ready(&config_path, &profile_config)
    {
        return Ok(PreparedProfile {
            environment: auth.base_url,
            state_dir: home
                .parent()
                .expect("profile home has state parent")
                .to_path_buf(),
            codex_home: home,
        });
    }

    fs::create_dir_all(&home)
        .with_context(|| format!("创建 Connector 工作区档案失败: {}", home.display()))?;
    set_private_directory(&home)?;
    let credential = baijimu_cli::create_llm_credential(workspace_id)
        .context("签发 Connector 工作区 LLM credential 失败")?;
    atomic_write_private(
        &auth_path,
        &serde_json::to_vec_pretty(&json!({
            "OPENAI_API_KEY": credential,
            "auth_mode": "apikey"
        }))?,
    )?;
    atomic_write_private(&config_path, workspace_config(&profile_config).as_bytes())?;
    let previous_created_at = read_metadata(&metadata_path)
        .filter(|metadata| {
            metadata.environment == auth.base_url && metadata.workspace_id == workspace_id
        })
        .map(|metadata| metadata.created_at_epoch_seconds)
        .unwrap_or_else(now_epoch_seconds);
    let metadata = WorkspaceProfileMetadata {
        schema_version: PROFILE_SCHEMA_VERSION,
        environment: auth.base_url,
        workspace_id,
        workspace_name: workspace.name,
        created_at_epoch_seconds: previous_created_at,
        updated_at_epoch_seconds: now_epoch_seconds(),
    };
    atomic_write_private(&metadata_path, &serde_json::to_vec_pretty(&metadata)?)?;
    Ok(PreparedProfile {
        environment: metadata.environment,
        state_dir: home
            .parent()
            .expect("profile home has state parent")
            .to_path_buf(),
        codex_home: home,
    })
}

pub(crate) fn state_dir(environment: &str, workspace_id: u64) -> PathBuf {
    connector_home()
        .join("workspace-profiles")
        .join(hex_encode(environment.as_bytes()))
        .join(workspace_id.to_string())
}

fn profile_home(environment: &str, workspace_id: u64) -> PathBuf {
    state_dir(environment, workspace_id).join("codex-home")
}

fn metadata_matches(path: &Path, environment: &str, workspace_id: u64) -> bool {
    read_metadata(path).is_some_and(|metadata| {
        metadata.schema_version == PROFILE_SCHEMA_VERSION
            && metadata.environment == environment
            && metadata.workspace_id == workspace_id
    })
}

fn read_metadata(path: &Path) -> Option<WorkspaceProfileMetadata> {
    fs::read(path)
        .ok()
        .and_then(|bytes| crate::json_compat::from_slice(&bytes).ok())
}

fn auth_file_ready(path: &Path) -> bool {
    fs::read(path)
        .ok()
        .and_then(|bytes| crate::json_compat::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("OPENAI_API_KEY")
                .and_then(|item| item.as_str())
                .map(str::to_string)
        })
        .is_some_and(|credential| !credential.trim().is_empty())
}

fn config_file_ready(path: &Path, config: &WorkspaceProfileConfig) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.parse::<DocumentMut>().ok())
        .is_some_and(|document| {
            document.get("model_provider").and_then(Item::as_str)
                == Some(config.router_provider.as_str())
                && document
                    .get("model_providers")
                    .and_then(Item::as_table)
                    .and_then(|providers| providers.get(config.router_provider.as_str()))
                    .and_then(Item::as_table)
                    .and_then(|provider| provider.get("base_url"))
                    .and_then(Item::as_str)
                    == Some(config.router_base_url.as_str())
        })
}

fn workspace_profile_config() -> Result<WorkspaceProfileConfig> {
    let config = crate::json_compat::from_slice::<WorkspaceProfileConfig>(include_bytes!(
        "../config/codex-workspace-profile.json"
    ))
    .context("解析 Connector 工作区档案配置失败")?;
    if config.schema_version != PROFILE_CONFIG_SCHEMA_VERSION
        || config.default_model.trim().is_empty()
        || config.router_provider.trim().is_empty()
        || !config.router_base_url.starts_with("https://")
    {
        bail!("Connector 工作区档案配置无效");
    }
    Ok(config)
}

fn workspace_config(config: &WorkspaceProfileConfig) -> String {
    let mut document = DocumentMut::new();
    document["model"] = value(config.default_model.as_str());
    document["model_provider"] = value(config.router_provider.as_str());
    document["sandbox_mode"] = value("danger-full-access");
    document["approval_policy"] = value("on-request");
    document["cli_auth_credentials_store"] = value("file");
    document["forced_login_method"] = value("api");
    document["model_providers"] = Item::Table(Table::new());
    document["model_providers"][config.router_provider.as_str()] = Item::Table(Table::new());
    let provider = &mut document["model_providers"][config.router_provider.as_str()];
    provider["name"] = value(config.router_provider.as_str());
    provider["base_url"] = value(config.router_base_url.as_str());
    provider["wire_api"] = value("responses");
    provider["requires_openai_auth"] = value(true);
    document.to_string()
}

fn atomic_write_private(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("工作区档案文件缺少父目录")?;
    fs::create_dir_all(parent)?;
    set_private_directory(parent)?;
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        now_epoch_seconds()
    ));
    fs::write(&temporary, content)
        .with_context(|| format!("写入 Connector 工作区档案失败: {}", temporary.display()))?;
    set_private_file(&temporary)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&temporary, path)
        .with_context(|| format!("提交 Connector 工作区档案失败: {}", path.display()))?;
    set_private_file(path)
}

fn hex_encode(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn set_private_directory(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_file(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_path_encoding_is_lossless() {
        let source = b"https://api.example.test/v1";
        let encoded = hex_encode(source);
        assert_eq!(encoded.len(), source.len() * 2);
        assert!(encoded.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn generated_config_is_workspace_router_config() {
        let profile = workspace_profile_config().unwrap();
        let config = workspace_config(&profile).parse::<DocumentMut>().unwrap();
        assert_eq!(
            config["model_provider"].as_str(),
            Some(profile.router_provider.as_str())
        );
        assert_eq!(
            config["model_providers"][profile.router_provider.as_str()]["base_url"].as_str(),
            Some(profile.router_base_url.as_str())
        );
    }
}
