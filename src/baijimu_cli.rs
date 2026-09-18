use anyhow::{bail, Context, Result};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const BAIJIMU_BINARY_ENV: &str = "CODEX_CONNECTOR_BAIJIMU_BINARY";

pub fn command() -> Result<Command> {
    Ok(Command::new(binary()?))
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

#[cfg(test)]
mod tests {
    use super::*;

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
