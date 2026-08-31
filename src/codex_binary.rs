use semver::Version;
use serde_json::{json, Value};
use std::fmt;
use std::io;
use std::process::Command;
use std::thread;
use std::time::Duration;

pub const COMMAND: &str = "codex";

#[derive(Clone, Debug, Default)]
pub struct CliInspection {
    pub version: Option<String>,
    pub app_server_supported: bool,
    pub error: Option<String>,
}

impl CliInspection {
    pub fn semantic_version(&self) -> Option<Version> {
        self.version.as_deref().and_then(parse_version_output)
    }

    pub fn satisfies(&self, required: &Version) -> bool {
        self.app_server_supported
            && self
                .semantic_version()
                .is_some_and(|installed| installed >= *required)
    }

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

fn parse_version_output(output: &str) -> Option<Version> {
    output
        .split_whitespace()
        .find_map(|token| Version::parse(token.trim_start_matches('v')).ok())
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
    let version_output = command_output(command, &["--version"]).map_err(|error| CommandError {
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

    match command_output(command, &["app-server", "--help"]) {
        Ok(output) if output.status.success() => {
            #[cfg(unix)]
            {
                match command_output(command, &["app-server", "proxy", "--help"]) {
                    Ok(output) if output.status.success() => Ok(CliInspection {
                        version,
                        app_server_supported: true,
                        error: None,
                    }),
                    Ok(output) => Ok(CliInspection {
                        version,
                        app_server_supported: false,
                        error: Some(format!(
                            "codex app-server proxy --help 退出状态为 {}",
                            output.status
                        )),
                    }),
                    Err(error) => Ok(CliInspection {
                        version,
                        app_server_supported: false,
                        error: Some(format!("验证 codex app-server proxy 失败：{error}")),
                    }),
                }
            }
            #[cfg(not(unix))]
            Ok(CliInspection {
                version,
                app_server_supported: true,
                error: None,
            })
        }
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

fn command_output(command: &str, arguments: &[&str]) -> io::Result<std::process::Output> {
    const MAX_ATTEMPTS: usize = 5;
    for attempt in 1..=MAX_ATTEMPTS {
        match Command::new(command).args(arguments).output() {
            Err(error) if is_transient_executable_busy(&error) && attempt < MAX_ATTEMPTS => {
                thread::sleep(Duration::from_millis(25));
            }
            result => return result,
        }
    }
    unreachable!("bounded command retry loop always returns")
}

fn is_transient_executable_busy(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::ETXTBSY)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::env;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::path::PathBuf;
    #[cfg(unix)]
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
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
        let temporary = root.join("codex.tmp");
        let mut file = fs::File::create(&temporary).expect("create command");
        file.write_all(source.as_bytes()).expect("write command");
        file.sync_all().expect("sync command");
        drop(file);
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755)).expect("chmod command");
        fs::rename(temporary, &command).expect("publish command");
        command
    }

    #[cfg(unix)]
    #[test]
    fn inspects_the_same_command_for_version_and_app_server() {
        let command = command_script(
            "supported",
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 1.2.3'; exit 0; fi\nif [ \"$1\" = \"app-server\" ] && [ \"$2\" = \"--help\" ]; then exit 0; fi\nif [ \"$1\" = \"app-server\" ] && [ \"$2\" = \"proxy\" ] && [ \"$3\" = \"--help\" ]; then exit 0; fi\nexit 2\n",
        );

        let inspection = inspect_command(command.to_str().expect("utf8 path")).expect("inspect");

        assert_eq!(inspection.version.as_deref(), Some("codex-cli 1.2.3"));
        assert!(inspection.app_server_supported);
        fs::remove_dir_all(command.parent().expect("command parent")).expect("remove test root");
    }

    #[test]
    fn parses_stable_and_prerelease_cli_versions() {
        assert_eq!(
            parse_version_output("codex-cli 0.149.0"),
            Some(Version::new(0, 149, 0))
        );
        assert_eq!(
            parse_version_output("codex-cli 0.149.0-alpha.4.1"),
            Version::parse("0.149.0-alpha.4.1").ok()
        );
        assert_eq!(parse_version_output("codex-cli unknown"), None);
    }

    #[cfg(unix)]
    #[test]
    fn recognizes_linux_text_file_busy_as_a_transient_execution_error() {
        assert!(is_transient_executable_busy(&io::Error::from_raw_os_error(
            libc::ETXTBSY
        )));
        assert!(!is_transient_executable_busy(
            &io::Error::from_raw_os_error(libc::ENOENT)
        ));
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
