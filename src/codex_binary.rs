use serde_json::{json, Value};
use std::fmt;
use std::process::Command;

pub const COMMAND: &str = "codex";

#[derive(Clone, Debug, Default)]
pub struct CliInspection {
    pub version: Option<String>,
    pub app_server_supported: bool,
    pub error: Option<String>,
}

impl CliInspection {
    pub fn status_value(&self) -> Value {
        json!({
            "mode": "path",
            "resolved": COMMAND,
            "source": "process_path",
            "checkedPaths": [],
            "version": self.version,
            "appServerSupported": self.app_server_supported,
            "inspectionError": self.error,
            "error": null,
        })
    }
}

#[derive(Clone, Debug)]
pub struct CommandError {
    pub reason: String,
}

impl CommandError {
    pub fn status_value(&self) -> Value {
        json!({
            "mode": "path",
            "resolved": COMMAND,
            "source": "process_path",
            "checkedPaths": [],
            "version": null,
            "appServerSupported": null,
            "inspectionError": null,
            "error": self.to_string(),
        })
    }

    pub fn data_value(&self) -> Value {
        json!({
            "command": COMMAND,
            "reason": self.reason,
        })
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "无法通过当前进程 PATH 执行 Codex CLI 命令“{COMMAND}”（{}）。请确认宿主已向连接器注入当前用户 PATH",
            self.reason
        )
    }
}

impl std::error::Error for CommandError {}

pub fn inspect() -> Result<CliInspection, CommandError> {
    inspect_command(COMMAND)
}

fn inspect_command(command: &str) -> Result<CliInspection, CommandError> {
    let version_output = Command::new(command)
        .arg("--version")
        .output()
        .map_err(|error| CommandError {
            reason: format!("执行 codex --version 失败：{error}"),
        })?;
    if !version_output.status.success() {
        return Err(CommandError {
            reason: format!("codex --version 退出状态为 {}", version_output.status),
        });
    }
    let stdout = String::from_utf8_lossy(&version_output.stdout)
        .trim()
        .to_string();
    let stderr = String::from_utf8_lossy(&version_output.stderr)
        .trim()
        .to_string();
    let version =
        Some(if stdout.is_empty() { stderr } else { stdout }).filter(|value| !value.is_empty());

    match Command::new(command)
        .args(["app-server", "--help"])
        .output()
    {
        Ok(output) if output.status.success() => Ok(CliInspection {
            version,
            app_server_supported: true,
            error: None,
        }),
        Ok(output) => Ok(CliInspection {
            version,
            app_server_supported: false,
            error: Some(format!(
                "codex app-server --help 退出状态为 {}",
                output.status
            )),
        }),
        Err(error) => Ok(CliInspection {
            version,
            app_server_supported: false,
            error: Some(format!("验证 codex app-server 失败：{error}")),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "baijimu-codex-command-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    #[cfg(unix)]
    fn command_script(name: &str, source: &str) -> PathBuf {
        let root = test_root(name);
        fs::create_dir_all(&root).expect("create test root");
        let command = root.join("codex");
        fs::write(&command, source).expect("write command");
        fs::set_permissions(&command, fs::Permissions::from_mode(0o755)).expect("chmod command");
        command
    }

    #[cfg(unix)]
    #[test]
    fn inspects_the_same_command_for_version_and_app_server() {
        let command = command_script(
            "supported",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 1.2.3'; exit 0; fi\nif [ \"$1\" = \"app-server\" ] && [ \"$2\" = \"--help\" ]; then exit 0; fi\nexit 2\n",
        );

        let inspection = inspect_command(command.to_str().expect("utf8 path")).expect("inspect");

        assert_eq!(inspection.version.as_deref(), Some("codex-cli 1.2.3"));
        assert!(inspection.app_server_supported);
        fs::remove_dir_all(command.parent().expect("command parent")).expect("remove test root");
    }

    #[cfg(unix)]
    #[test]
    fn reports_app_server_capability_failure_without_selecting_another_command() {
        let command = command_script(
            "unsupported",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 1.2.3'; exit 0; fi\nexit 3\n",
        );

        let inspection = inspect_command(command.to_str().expect("utf8 path")).expect("inspect");

        assert!(!inspection.app_server_supported);
        assert!(inspection
            .error
            .as_deref()
            .is_some_and(|error| error.contains("退出状态")));
        fs::remove_dir_all(command.parent().expect("command parent")).expect("remove test root");
    }

    #[test]
    fn unavailable_command_error_names_the_path_contract() {
        let error = inspect_command("baijimu-codex-command-that-does-not-exist")
            .expect_err("missing command must fail");

        assert!(error.to_string().contains("当前进程 PATH"));
        assert_eq!(error.data_value()["command"], COMMAND);
    }
}
