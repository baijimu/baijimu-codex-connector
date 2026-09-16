//! Device filesystem preparation. Platform employee/session authorization is
//! completed before Relay dispatch; workspace comes only from the trusted header.
use crate::{project_checkout, HttpError};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub(crate) enum PrepareProject {
    LocalDirectory {
        directory: String,
    },
    NewLocalDirectory {
        parent: String,
        name: String,
    },
    WorkspaceProject {
        #[serde(rename = "projectId")]
        project_id: u64,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparedProject {
    pub directory: String,
    pub project_id: Option<u64>,
}

pub(crate) fn prepare(
    workspace: u64,
    request: PrepareProject,
) -> Result<PreparedProject, HttpError> {
    if workspace == 0 {
        return Err(HttpError::new(400, "缺少可信工作区"));
    }
    match request {
        PrepareProject::LocalDirectory { directory } => local_directory(&directory),
        PrepareProject::NewLocalDirectory { parent, name } => create_directory(&parent, &name),
        PrepareProject::WorkspaceProject { project_id } => {
            if project_id == 0 {
                return Err(HttpError::new(400, "projectId 必须大于零"));
            }
            let result = project_checkout::prepare(project_checkout::CheckoutRequest {
                workspace_id: workspace,
                project_id,
                branch: None,
            })
            .map_err(|error| HttpError::internal(error.to_string()))?;
            Ok(PreparedProject {
                directory: result.directory,
                project_id: Some(project_id),
            })
        }
    }
}

fn absolute_directory(value: &str) -> Result<PathBuf, HttpError> {
    let path = Path::new(value);
    if !path.is_absolute()
        || value.len() > 1024
        || value.chars().any(char::is_control)
        || value.trim() != value
    {
        return Err(HttpError::new(400, "必须提供设备上的绝对目录路径"));
    }
    let directory = fs::canonicalize(path)
        .map_err(|error| HttpError::new(400, format!("设备目录不可访问: {error}")))?;
    if !directory.is_dir() {
        return Err(HttpError::new(400, "所选路径不是目录"));
    }
    Ok(directory)
}

fn local_directory(value: &str) -> Result<PreparedProject, HttpError> {
    let directory = absolute_directory(value)?;
    let text = directory
        .to_str()
        .ok_or_else(|| HttpError::new(400, "目录路径不是有效 UTF-8"))?;
    if text.len() > 1024 {
        return Err(HttpError::new(400, "目录路径超过会话长度限制"));
    }
    Ok(PreparedProject {
        directory: text.into(),
        project_id: None,
    })
}

fn create_directory(parent: &str, name: &str) -> Result<PreparedProject, HttpError> {
    let mut parts = Path::new(name).components();
    if !matches!(parts.next(), Some(Component::Normal(_)))
        || parts.next().is_some()
        || name.contains(['/', '\\'])
        || name.chars().any(char::is_control)
        || name.trim() != name
        || name.contains(':')
    {
        return Err(HttpError::new(400, "新项目名称必须是单个目录名"));
    }
    let parent = absolute_directory(parent)?;
    let destination = parent.join(name);
    if destination.to_str().is_none_or(|value| value.len() > 1024) {
        return Err(HttpError::new(
            400,
            "目录路径不是有效 UTF-8 或超过会话长度限制",
        ));
    }
    // Never overwrite or silently adopt existing contents after a lost response.
    fs::create_dir(&destination).map_err(|error| {
        HttpError::new(
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                409
            } else {
                400
            },
            format!("创建项目目录失败: {error}"),
        )
    })?;
    local_directory(
        destination
            .to_str()
            .ok_or_else(|| HttpError::new(400, "目录路径不是有效 UTF-8"))?,
    )
}

#[cfg(test)]
mod tests;
