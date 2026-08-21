use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const BAIJIMU_BINARY_ENV: &str = "CODEX_CONNECTOR_BAIJIMU_BINARY";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthStatus {
    pub authenticated: bool,
    pub base_url: String,
    pub current_workspace_id: Option<u64>,
    pub credential_workspace_ids: Vec<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatusContract {
    authenticated: bool,
    base_url: String,
    current_workspace_id: Option<u64>,
    workspace_ids: Vec<u64>,
}

pub fn command() -> Result<Command> {
    Ok(Command::new(binary()?))
}

pub fn auth_status() -> Result<AuthStatus> {
    let contract: AuthStatusContract = run_json("auth status", &["auth", "status"])?;
    if contract.authenticated && contract.workspace_ids.is_empty() {
        bail!("baijimu CLI 报告已认证，但授权工作区为空");
    }
    Ok(AuthStatus {
        authenticated: contract.authenticated,
        base_url: required_text(contract.base_url, "auth status.baseUrl")?,
        current_workspace_id: contract.current_workspace_id.filter(|id| *id > 0),
        credential_workspace_ids: positive_unique_ids(
            contract.workspace_ids,
            "auth status.workspaceIds",
        )?,
    })
}

fn binary() -> Result<PathBuf> {
    let value = env::var_os(BAIJIMU_BINARY_ENV)
        .filter(|value| !value.is_empty())
        .context("Bridge Agent 未注入平台管理的 baijimu CLI 绝对路径；请升级或重启 Bridge Agent")?;
    validate_binary_path(PathBuf::from(value))
}

fn validate_binary_path(path: PathBuf) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("{BAIJIMU_BINARY_ENV} 必须是绝对路径，不能依赖 PATH 查找")
    }
    if !Path::new(&path).is_file() {
        bail!("Bridge Agent 注入的 baijimu CLI 不存在：{}", path.display())
    }
    Ok(path)
}

fn run_json<T>(operation: &str, args: &[&str]) -> Result<T>
where
    T: DeserializeOwned,
{
    let output = command()?
        .args(args)
        .output()
        .with_context(|| format!("启动 baijimu CLI {operation} 失败；请检查平台管理的 CLI 安装"))?;
    if !output.status.success() {
        let detail = compact_error(&String::from_utf8_lossy(&output.stderr));
        bail!(
            "baijimu CLI {operation} 失败{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!("：{detail}")
            }
        );
    }
    crate::json_compat::from_slice(&output.stdout)
        .with_context(|| format!("baijimu CLI {operation} 未返回合法 JSON"))
}

fn required_text(value: String, field: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("baijimu CLI 响应缺少 {field}");
    }
    Ok(value.to_string())
}

fn positive_unique_ids(mut ids: Vec<u64>, field: &str) -> Result<Vec<u64>> {
    if ids.contains(&0) {
        bail!("baijimu CLI 响应中的 {field} 包含非法 ID");
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

fn compact_error(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(400)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_status_contract_requires_owned_fields_and_normalizes_ids() {
        let contract: AuthStatusContract = crate::json_compat::from_slice(
            br#"{
                "authenticated": true,
                "baseUrl": "https://api.baijimu.com",
                "configuredCurrentWorkspaceId": 642,
                "credentialCount": 2,
                "currentWorkspaceId": 642,
                "sharedAuthPath": "owned-by-baijimu-cli",
                "verification": null,
                "workspaceIds": [1390, 642, 642]
            }"#,
        )
        .unwrap();
        let status = AuthStatus {
            authenticated: contract.authenticated,
            base_url: required_text(contract.base_url, "baseUrl").unwrap(),
            current_workspace_id: contract.current_workspace_id,
            credential_workspace_ids: positive_unique_ids(contract.workspace_ids, "workspaceIds")
                .unwrap(),
        };
        assert_eq!(status.credential_workspace_ids, vec![642, 1390]);
        assert_eq!(status.current_workspace_id, Some(642));
    }

    #[test]
    fn invalid_workspace_ids_fail_closed() {
        assert!(positive_unique_ids(vec![642, 0], "workspaceIds").is_err());
    }

    #[test]
    fn managed_cli_path_requires_an_absolute_existing_file() {
        assert!(validate_binary_path(PathBuf::from("baijimu")).is_err());
        let executable = std::env::current_exe().unwrap();
        assert_eq!(
            validate_binary_path(executable.clone()).unwrap(),
            executable
        );
    }
}
