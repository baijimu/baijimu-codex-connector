use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutRequest {
    pub workspace_id: u64,
    pub project_id: u64,
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutResult {
    pub workspace_id: u64,
    pub project_id: u64,
    pub directory: String,
    pub branch: String,
    pub remote_url: String,
    pub reused: bool,
    pub checkout: Value,
}

pub fn prepare(request: CheckoutRequest) -> Result<CheckoutResult> {
    if request.workspace_id == 0 || request.project_id == 0 {
        bail!("workspaceId和projectId必须大于0");
    }
    let root = projects_root()?;
    let directory = root
        .join(format!("workspace-{}", request.workspace_id))
        .join(format!("project-{}", request.project_id));
    if directory.exists() {
        return inspect_existing(&directory, &request);
    }
    fs::create_dir_all(directory.parent().context("平台项目检出目录缺少父目录")?)
        .with_context(|| format!("创建平台项目目录失败: {}", directory.display()))?;
    let mut command =
        crate::baijimu_cli::command().context("Bridge Agent 未提供平台管理的 baijimu CLI")?;
    command
        .args([
            "project",
            "checkout",
            &request.project_id.to_string(),
            "--workspace-id",
            &request.workspace_id.to_string(),
            "--directory",
        ])
        .arg(&directory);
    if let Some(branch) = request
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        command.args(["--branch", branch]);
    }
    let output = command
        .output()
        .context("启动 baijimu project checkout 失败；请先安装平台管理的 baijimu CLI")?;
    if !output.status.success() {
        bail!(
            "平台项目检出失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let checkout: Value = crate::json_compat::from_slice(&output.stdout)
        .context("baijimu project checkout 未返回合法JSON")?;
    let canonical_directory = fs::canonicalize(&directory)
        .with_context(|| format!("检出目录回查失败: {}", directory.display()))?;
    let branch = required_json_string(&checkout, "branch")?;
    let remote_url = required_json_string(&checkout, "remoteUrl")?;
    Ok(CheckoutResult {
        workspace_id: request.workspace_id,
        project_id: request.project_id,
        directory: canonical_directory.display().to_string(),
        branch,
        remote_url,
        reused: false,
        checkout,
    })
}

fn inspect_existing(directory: &Path, request: &CheckoutRequest) -> Result<CheckoutResult> {
    let workspace_id = git_output(directory, &["config", "--get", "baijimu.workspaceId"])?;
    let project_id = git_output(directory, &["config", "--get", "baijimu.projectId"])?;
    if workspace_id != request.workspace_id.to_string()
        || project_id != request.project_id.to_string()
    {
        bail!(
            "目标目录已存在但不属于请求的平台项目，拒绝复用: {}",
            directory.display()
        );
    }
    let remote_url = git_output(directory, &["remote", "get-url", "origin"])?;
    let expected_path = format!(
        "/git/v1/workspaces/{}/projects/{}.git",
        request.workspace_id, request.project_id
    );
    let parsed = reqwest::Url::parse(&remote_url).context("现有项目 origin URL 无效")?;
    if parsed.path() != expected_path {
        bail!("现有项目 origin 与请求的平台项目不一致，拒绝复用");
    }
    let branch = git_output(directory, &["branch", "--show-current"])?;
    if !branch.starts_with("codex/") {
        bail!("现有项目当前分支不在 codex 命名空间，拒绝作为 Codex 工作区");
    }
    if let Some(requested) = request
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let suffix_matches =
            !requested.starts_with("codex/") && branch.rsplit('/').next() == Some(requested);
        if branch != requested && !suffix_matches {
            bail!("现有项目分支与请求分支不一致: current={branch}, requested={requested}");
        }
    }
    let canonical_directory = fs::canonicalize(directory)
        .with_context(|| format!("现有项目目录回查失败: {}", directory.display()))?;
    Ok(CheckoutResult {
        workspace_id: request.workspace_id,
        project_id: request.project_id,
        directory: canonical_directory.display().to_string(),
        branch: branch.clone(),
        remote_url: remote_url.clone(),
        reused: true,
        checkout: json!({
            "workspaceId": request.workspace_id,
            "projectId": request.project_id,
            "directory": canonical_directory,
            "branch": branch,
            "remoteUrl": remote_url,
        }),
    })
}

fn git_output(directory: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("执行 git {} 失败", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "现有目录不是有效的平台 Git checkout: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn required_json_string(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .with_context(|| format!("baijimu project checkout 响应缺少{key}"))
}

fn projects_root() -> Result<PathBuf> {
    let root = env::var_os("CODEX_CONNECTOR_PROJECTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| super::home_dir().join("Baijimu").join("Projects"));
    if !root.is_absolute() {
        bail!("CODEX_CONNECTOR_PROJECTS_DIR必须是绝对路径");
    }
    Ok(root)
}
