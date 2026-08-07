use serde_json::{json, Value};
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Clone, Debug)]
pub struct Resolution {
    pub requested: Option<String>,
    pub path: PathBuf,
    pub source: &'static str,
    pub checked_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct CliInspection {
    pub version: Option<String>,
    pub app_server_supported: bool,
    pub error: Option<String>,
}

impl Resolution {
    pub fn status_value(&self, inspection: Option<&CliInspection>) -> Value {
        json!({
            "requested": self.requested,
            "mode": if self.requested.is_some() { "advanced_override" } else { "auto" },
            "resolved": self.path,
            "source": self.source,
            "checkedPaths": display_paths(&self.checked_paths),
            "version": inspection.and_then(|value| value.version.clone()),
            "appServerSupported": inspection.map(|value| value.app_server_supported),
            "inspectionError": inspection.and_then(|value| value.error.clone()),
            "error": null,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ResolutionError {
    pub requested: Option<String>,
    pub checked_paths: Vec<PathBuf>,
    pub reason: String,
}

impl ResolutionError {
    pub fn status_value(&self) -> Value {
        json!({
            "requested": self.requested,
            "mode": if self.requested.is_some() { "advanced_override" } else { "auto" },
            "resolved": null,
            "source": null,
            "checkedPaths": display_paths(&self.checked_paths),
            "version": null,
            "appServerSupported": null,
            "inspectionError": null,
            "error": self.to_string(),
        })
    }

    pub fn data_value(&self) -> Value {
        json!({
            "requested": self.requested,
            "checkedPaths": display_paths(&self.checked_paths),
            "reason": self.reason,
        })
    }
}

impl fmt::Display for ResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Codex CLI was not found or is not executable ({}). Install the official Codex CLI, or set the advanced CODEX_CONNECTOR_CODEX_BINARY override to an absolute executable path",
            self.reason
        )
    }
}

impl std::error::Error for ResolutionError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    MacOs,
    Linux,
    Windows,
    Other,
}

#[derive(Clone, Debug)]
struct ResolverContext {
    platform: Platform,
    home: Option<PathBuf>,
    local_app_data: Option<PathBuf>,
    path: Option<OsString>,
    path_ext: Vec<String>,
    shell: Option<PathBuf>,
}

impl ResolverContext {
    fn from_env() -> Self {
        Self {
            platform: current_platform(),
            home: home_dir(),
            local_app_data: env::var_os("LOCALAPPDATA").map(PathBuf::from),
            path: env::var_os("PATH"),
            path_ext: windows_command_extensions_from(env::var("PATHEXT").ok().as_deref()),
            shell: env::var_os("SHELL").map(PathBuf::from),
        }
    }
}

pub fn resolve(requested: Option<&str>) -> Result<Resolution, ResolutionError> {
    resolve_with_context(requested, &ResolverContext::from_env(), true)
}

pub fn inspect(resolution: &Resolution) -> CliInspection {
    let version_output = Command::new(&resolution.path).arg("--version").output();
    let version = match version_output {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let fallback = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Some(if value.is_empty() { fallback } else { value }).filter(|value| !value.is_empty())
        }
        Ok(output) => {
            return CliInspection {
                error: Some(format!("codex --version exited with {}", output.status)),
                ..CliInspection::default()
            };
        }
        Err(error) => {
            return CliInspection {
                error: Some(format!("failed to run codex --version: {error}")),
                ..CliInspection::default()
            };
        }
    };
    match Command::new(&resolution.path)
        .args(["app-server", "--help"])
        .output()
    {
        Ok(output) if output.status.success() => CliInspection {
            version,
            app_server_supported: true,
            error: None,
        },
        Ok(output) => CliInspection {
            version,
            app_server_supported: false,
            error: Some(format!(
                "codex app-server --help exited with {}",
                output.status
            )),
        },
        Err(error) => CliInspection {
            version,
            app_server_supported: false,
            error: Some(format!("failed to verify codex app-server: {error}")),
        },
    }
}

fn resolve_with_context(
    requested: Option<&str>,
    context: &ResolverContext,
    search_login_environment: bool,
) -> Result<Resolution, ResolutionError> {
    let mut checked_paths = Vec::new();
    if let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        if !Path::new(requested).is_absolute() {
            return Err(ResolutionError {
                requested: Some(requested.to_string()),
                checked_paths,
                reason: "the advanced override is not an absolute path".to_string(),
            });
        }
        let path = PathBuf::from(requested);
        checked_paths.push(path.clone());
        for candidate in explicit_path_candidates(&path, context) {
            if !checked_paths.contains(&candidate) {
                checked_paths.push(candidate.clone());
            }
            if is_launchable_file(&candidate, context.platform) {
                return Ok(Resolution {
                    requested: Some(requested.to_string()),
                    path: candidate,
                    source: "explicit_path",
                    checked_paths,
                });
            }
        }
        return Err(ResolutionError {
            requested: Some(requested.to_string()),
            checked_paths,
            reason: if context.platform == Platform::Windows {
                "the explicitly configured path is unavailable or is not a supported Windows .exe/.com/.bat/.cmd launcher".to_string()
            } else {
                "the explicitly configured path is unavailable".to_string()
            },
        });
    }

    let command = "codex";

    for path in path_candidates(command, context) {
        if let Some(resolution) = check_candidate(
            None,
            path,
            "process_path",
            context.platform,
            &mut checked_paths,
        ) {
            return Ok(resolution);
        }
    }

    for (path, source) in known_codex_candidates(context) {
        if let Some(resolution) =
            check_candidate(None, path, source, context.platform, &mut checked_paths)
        {
            return Ok(resolution);
        }
    }

    if search_login_environment {
        if let Some(path) = resolve_from_login_environment(command, context) {
            if let Some(resolution) = check_candidate(
                None,
                path,
                "login_environment",
                context.platform,
                &mut checked_paths,
            ) {
                return Ok(resolution);
            }
        }
    }

    Err(ResolutionError {
        requested: None,
        checked_paths,
        reason: "it was absent from the process PATH, official CLI install locations, and the user login environment".to_string(),
    })
}

fn check_candidate(
    requested: Option<&str>,
    path: PathBuf,
    source: &'static str,
    platform: Platform,
    checked_paths: &mut Vec<PathBuf>,
) -> Option<Resolution> {
    if !checked_paths.contains(&path) {
        checked_paths.push(path.clone());
    }
    is_launchable_file(&path, platform).then(|| Resolution {
        requested: requested.map(str::to_string),
        path,
        source,
        checked_paths: checked_paths.clone(),
    })
}

fn explicit_path_candidates(path: &Path, context: &ResolverContext) -> Vec<PathBuf> {
    if context.platform != Platform::Windows || path.extension().is_some() {
        return vec![path.to_path_buf()];
    }
    context
        .path_ext
        .iter()
        .map(|extension| append_extension(path, extension))
        .collect()
}

fn append_extension(path: &Path, extension: &str) -> PathBuf {
    let mut candidate = path.as_os_str().to_os_string();
    candidate.push(extension);
    PathBuf::from(candidate)
}

fn path_candidates(requested: &str, context: &ResolverContext) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let Some(path) = context.path.as_ref() else {
        return candidates;
    };
    for directory in env::split_paths(path) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        if context.platform == Platform::Windows {
            let requested_path = Path::new(requested);
            if requested_path.extension().is_some() {
                candidates.push(directory.join(requested));
            } else {
                for extension in &context.path_ext {
                    candidates.push(directory.join(format!("{requested}{extension}")));
                }
            }
        } else {
            candidates.push(directory.join(requested));
        }
    }
    candidates
}

fn known_codex_candidates(context: &ResolverContext) -> Vec<(PathBuf, &'static str)> {
    let mut candidates = Vec::new();
    if let Some(home) = &context.home {
        let binary = if context.platform == Platform::Windows {
            "codex.exe"
        } else {
            "codex"
        };
        candidates.push((
            home.join(".local").join("bin").join(binary),
            "official_user_install",
        ));
    }
    match context.platform {
        Platform::MacOs => {
            candidates.extend([
                (
                    PathBuf::from("/opt/homebrew/bin/codex"),
                    "official_system_install",
                ),
                (
                    PathBuf::from("/usr/local/bin/codex"),
                    "official_system_install",
                ),
            ]);
        }
        Platform::Linux => {
            candidates.extend([
                (
                    PathBuf::from("/usr/local/bin/codex"),
                    "official_system_install",
                ),
                (PathBuf::from("/usr/bin/codex"), "official_system_install"),
                (PathBuf::from("/snap/bin/codex"), "official_system_install"),
            ]);
        }
        Platform::Windows => {
            if let Some(local_app_data) = &context.local_app_data {
                if let Some(binary) = managed_windows_cli(local_app_data) {
                    candidates.push((binary, "connector_managed_official_cli"));
                }
            }
        }
        Platform::Other => {}
    }
    unique_candidates(candidates)
}

fn managed_windows_cli(local_app_data: &Path) -> Option<PathBuf> {
    let state_path = local_app_data
        .join("OpenAI")
        .join("Codex")
        .join("cli")
        .join("current.json");
    let state = fs::read_to_string(state_path).ok()?;
    let value = serde_json::from_str::<Value>(state.trim_start_matches('\u{feff}')).ok()?;
    value
        .get("binaryPath")
        .and_then(Value::as_str)
        .filter(|value| Path::new(value).is_absolute())
        .map(PathBuf::from)
}

fn resolve_from_login_environment(requested: &str, context: &ResolverContext) -> Option<PathBuf> {
    match context.platform {
        Platform::Windows => resolve_from_windows_command(requested),
        Platform::MacOs | Platform::Linux | Platform::Other => {
            let shell = context.shell.clone().unwrap_or_else(|| {
                if context.platform == Platform::MacOs {
                    PathBuf::from("/bin/zsh")
                } else {
                    PathBuf::from("/bin/sh")
                }
            });
            if !shell.is_absolute() || !shell.exists() {
                return None;
            }
            let output = Command::new(shell)
                .args([
                    "-lc",
                    "command -v -- \"$1\"",
                    "baijimu-codex-resolver",
                    requested,
                ])
                .output()
                .ok()?;
            output
                .status
                .success()
                .then(|| first_output_path(&output.stdout))
        }
    }
}

fn resolve_from_windows_command(requested: &str) -> Option<PathBuf> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "& { param($name) $command = Get-Command $name -ErrorAction SilentlyContinue; if ($command) { $command.Source } }",
            requested,
        ])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| first_output_path(&output.stdout))
}

fn first_output_path(stdout: &[u8]) -> PathBuf {
    let value = String::from_utf8_lossy(stdout);
    PathBuf::from(value.lines().next().unwrap_or_default().trim())
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn is_launchable_file(path: &Path, platform: Platform) -> bool {
    if platform == Platform::Windows && !has_supported_windows_extension(path) {
        return false;
    }
    is_executable_file(path)
}

fn has_supported_windows_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["com", "exe", "bat", "cmd"]
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(extension))
        })
}

fn windows_command_extensions_from(value: Option<&str>) -> Vec<String> {
    const SUPPORTED: [&str; 4] = [".COM", ".EXE", ".BAT", ".CMD"];
    let mut extensions = Vec::new();
    for extension in value
        .into_iter()
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .filter(|value| {
            SUPPORTED
                .iter()
                .any(|item| item.eq_ignore_ascii_case(value))
        })
    {
        if !extensions
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(extension))
        {
            extensions.push(extension.to_string());
        }
    }
    if extensions.is_empty() {
        extensions = SUPPORTED.iter().map(|value| (*value).to_string()).collect();
    }
    extensions
}

fn unique_candidates(paths: Vec<(PathBuf, &'static str)>) -> Vec<(PathBuf, &'static str)> {
    let mut unique = Vec::new();
    for candidate in paths {
        if !unique.iter().any(|(path, _)| path == &candidate.0) {
            unique.push(candidate);
        }
    }
    unique
}

fn display_paths(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn current_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "baijimu-codex-binary-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    fn executable(path: &Path) {
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, b"test").expect("write executable");
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod executable");
    }

    fn context(home: &Path, path: OsString) -> ResolverContext {
        ResolverContext {
            platform: Platform::Other,
            home: Some(home.to_path_buf()),
            local_app_data: None,
            path: Some(path),
            path_ext: vec![".EXE".to_string()],
            shell: None,
        }
    }

    fn windows_context(home: &Path, path: OsString) -> ResolverContext {
        ResolverContext {
            platform: Platform::Windows,
            home: Some(home.to_path_buf()),
            local_app_data: None,
            path: Some(path),
            path_ext: vec![".exe".to_string(), ".cmd".to_string()],
            shell: None,
        }
    }

    #[test]
    fn finds_installer_managed_user_binary_without_process_path() {
        let root = test_root("user-bin");
        let binary = root.join(".local/bin/codex");
        executable(&binary);

        let resolution = resolve_with_context(None, &context(&root, OsString::new()), false)
            .expect("resolve user binary");

        assert_eq!(resolution.path, binary);
        assert_eq!(resolution.source, "official_user_install");
        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn process_path_takes_precedence_over_known_locations() {
        let root = test_root("path-priority");
        let path_binary = root.join("path-bin/codex");
        let known_binary = root.join(".local/bin/codex");
        executable(&path_binary);
        executable(&known_binary);

        let resolution = resolve_with_context(
            None,
            &context(
                &root,
                env::join_paths([root.join("path-bin")]).expect("join PATH"),
            ),
            false,
        )
        .expect("resolve PATH binary");

        assert_eq!(resolution.path, path_binary);
        assert_eq!(resolution.source, "process_path");
        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn finds_connector_managed_official_windows_cli() {
        let root = test_root("windows-managed-cli");
        let local_app_data = root.join("local-app-data");
        let binary = local_app_data.join("OpenAI/Codex/cli/versions/sha256/codex.exe");
        executable(&binary);
        let state = local_app_data.join("OpenAI/Codex/cli/current.json");
        fs::create_dir_all(state.parent().expect("state parent")).expect("create state parent");
        fs::write(
            &state,
            serde_json::to_vec(&json!({ "binaryPath": binary })).expect("serialize state"),
        )
        .expect("write state");
        let mut resolver_context = context(&root, OsString::new());
        resolver_context.platform = Platform::Windows;
        resolver_context.local_app_data = Some(local_app_data);

        let resolution = resolve_with_context(None, &resolver_context, false)
            .expect("resolve managed official Windows CLI");

        assert_eq!(resolution.path, binary);
        assert_eq!(resolution.source, "connector_managed_official_cli");
        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn windows_path_ignores_extensionless_npm_shim_and_selects_cmd() {
        let root = test_root("windows-path-cmd");
        let bin = root.join("bin");
        executable(&bin.join("codex"));
        let launcher = bin.join("codex.cmd");
        executable(&launcher);

        let resolution = resolve_with_context(
            None,
            &windows_context(&root, env::join_paths([&bin]).expect("join PATH")),
            false,
        )
        .expect("resolve Windows cmd launcher");

        assert_eq!(resolution.path, launcher);
        assert_eq!(resolution.source, "process_path");
        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn windows_explicit_extensionless_bridge_path_selects_sibling_cmd() {
        let root = test_root("windows-explicit-cmd");
        let shim = root.join("workbuddy/codex");
        executable(&shim);
        let launcher = root.join("workbuddy/codex.cmd");
        executable(&launcher);

        let resolution = resolve_with_context(
            Some(shim.to_str().expect("utf-8 path")),
            &windows_context(&root, OsString::new()),
            false,
        )
        .expect("repair old Bridge Agent extensionless path");

        assert_eq!(resolution.path, launcher);
        assert_eq!(resolution.source, "explicit_path");
        assert!(resolution.checked_paths.contains(&shim));
        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn windows_explicit_extensionless_file_without_launcher_is_rejected() {
        let root = test_root("windows-explicit-reject");
        let shim = root.join("workbuddy/codex");
        executable(&shim);

        let error = resolve_with_context(
            Some(shim.to_str().expect("utf-8 path")),
            &windows_context(&root, OsString::new()),
            false,
        )
        .expect_err("extensionless Unix shim must not be launched on Windows");

        assert!(error.reason.contains("supported Windows"));
        assert!(error.checked_paths.contains(&shim));
        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn windows_command_extensions_exclude_non_native_script_types() {
        assert_eq!(
            windows_command_extensions_from(Some(".JS;.CMD;.cmd;.EXE;.PS1")),
            vec![".CMD".to_string(), ".EXE".to_string()]
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolved_windows_cmd_launcher_can_be_spawned() {
        let root = test_root("windows-cmd-spawn");
        let launcher = root.join("codex.cmd");
        fs::create_dir_all(&root).expect("create root");
        fs::write(&launcher, b"@echo off\r\necho %1\r\n").expect("write cmd launcher");
        let context = windows_context(&root, OsString::new());

        let resolution = resolve_with_context(
            Some(launcher.to_str().expect("utf-8 path")),
            &context,
            false,
        )
        .expect("resolve cmd launcher");
        let output = Command::new(&resolution.path)
            .arg("app-server")
            .output()
            .expect("spawn cmd launcher");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "app-server");
        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn rejects_non_absolute_advanced_override() {
        let root = test_root("relative-override");
        let error = resolve_with_context(Some("codex"), &context(&root, OsString::new()), false)
            .expect_err("advanced override must be absolute");

        assert!(error.reason.contains("absolute path"));
        assert_eq!(error.requested.as_deref(), Some("codex"));
    }

    #[test]
    fn invalid_explicit_path_does_not_fall_back() {
        let root = test_root("explicit-authoritative");
        executable(&root.join(".local/bin/codex"));
        let requested = root.join("missing/codex");

        let error = resolve_with_context(
            Some(requested.to_str().expect("utf-8 path")),
            &context(&root, OsString::new()),
            false,
        )
        .expect_err("explicit path must be authoritative");

        assert_eq!(error.checked_paths, vec![requested]);
        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn rejects_non_executable_unix_file() {
        let root = test_root("not-executable");
        let binary = root.join(".local/bin/codex");
        fs::create_dir_all(binary.parent().expect("parent")).expect("create parent");
        fs::write(&binary, b"test").expect("write file");
        #[cfg(unix)]
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o644)).expect("chmod file");

        let result = resolve_with_context(None, &context(&root, OsString::new()), false);

        #[cfg(unix)]
        assert!(result.is_err());
        #[cfg(not(unix))]
        assert!(result.is_ok());
        fs::remove_dir_all(root).expect("remove test root");
    }
}
