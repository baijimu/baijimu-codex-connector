use anyhow::Result;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct DesktopSwitch {
    #[cfg(any(windows, target_os = "macos"))]
    was_running: bool,
}

pub fn stop_for_codex_home_switch() -> Result<DesktopSwitch> {
    platform::stop_for_codex_home_switch()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn launch_and_verify(codex_home: &Path) -> Result<()> {
    platform::launch_and_verify(codex_home)
}

impl DesktopSwitch {
    pub fn restart_and_verify(&self, codex_home: &Path) -> Result<bool> {
        platform::restart_and_verify(self, codex_home)
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use anyhow::Context;
    use serde::Deserialize;
    use std::process::Command;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct StopResult {
        was_running: bool,
    }

    const STOP_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$packages = @('OpenAI.Codex', 'OpenAI.ChatGPT') | ForEach-Object { Get-AppxPackage -Name $_ -ErrorAction SilentlyContinue } | Where-Object { $_ }
if (-not $packages) {
  $packages = @(Get-AppxPackage -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'OpenAI.Codex*' -or ($_.Name -like 'OpenAI.ChatGPT*' -and $_.Name -notlike 'OpenAI.ChatGPT-Desktop*') })
}
$roots = @($packages | ForEach-Object { $_.InstallLocation } | Where-Object { $_ })
$targets = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
  try {
    $path = $_.Path
    if (-not $path) { return $false }
    return ($roots | Where-Object { $path.StartsWith($_, [System.StringComparison]::OrdinalIgnoreCase) }).Count -gt 0
  } catch { return $false }
})
$wasRunning = $targets.Count -gt 0
if ($wasRunning) {
  $targets | Stop-Process -Force -ErrorAction Stop
  $deadline = (Get-Date).AddSeconds(15)
  do {
    $remaining = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $targets.Id -contains $_.Id })
    if ($remaining.Count -gt 0) { Start-Sleep -Milliseconds 250 }
  } while ($remaining.Count -gt 0 -and (Get-Date) -lt $deadline)
  if ($remaining.Count -gt 0) { throw 'ChatGPT/Codex desktop processes did not stop within 15 seconds' }
}
[pscustomobject]@{ wasRunning = $wasRunning } | ConvertTo-Json -Compress
"#;

    const LAUNCH_AND_VERIFY_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$codexHome = $env:CODEX_HOME
if (-not $codexHome) { throw 'Explicit CODEX_HOME is required for isolated desktop launch' }
$packages = @('OpenAI.Codex', 'OpenAI.ChatGPT') | ForEach-Object { Get-AppxPackage -Name $_ -ErrorAction SilentlyContinue } | Where-Object { $_ }
if (-not $packages) {
  $packages = @(Get-AppxPackage -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'OpenAI.Codex*' -or ($_.Name -like 'OpenAI.ChatGPT*' -and $_.Name -notlike 'OpenAI.ChatGPT-Desktop*') })
}
if (-not $packages) { throw 'ChatGPT/Codex desktop package is not installed for the current user' }
$roots = @($packages | ForEach-Object { $_.InstallLocation } | Where-Object { $_ })
$entry = @($packages | ForEach-Object {
  $package = $_
  [xml]$manifest = Get-Content -LiteralPath (Join-Path $package.InstallLocation 'AppxManifest.xml')
  @($manifest.Package.Applications.Application | Where-Object { $_.Executable } | Select-Object -First 1) | ForEach-Object {
    [pscustomobject]@{ package = $package; executable = (Join-Path $package.InstallLocation ([string]$_.Executable)) }
  }
} | Select-Object -First 1)
if (-not $entry -or -not (Test-Path -LiteralPath $entry[0].executable)) { throw 'ChatGPT/Codex packaged desktop executable is unavailable' }
$existing = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
  try {
    $path = $_.Path
    if (-not $path) { return $false }
    return ($roots | Where-Object { $path.StartsWith($_, [System.StringComparison]::OrdinalIgnoreCase) }).Count -gt 0
  } catch { return $false }
})
if ($existing.Count -gt 0) {
  $existing | Stop-Process -Force -ErrorAction Stop
  $deadline = (Get-Date).AddSeconds(15)
  do {
    $remaining = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $existing.Id -contains $_.Id })
    if ($remaining.Count -gt 0) { Start-Sleep -Milliseconds 250 }
  } while ($remaining.Count -gt 0 -and (Get-Date) -lt $deadline)
  if ($remaining.Count -gt 0) { throw 'ChatGPT/Codex desktop processes did not stop within 15 seconds' }
}
Start-Process -FilePath $entry[0].executable -ErrorAction Stop
$deadline = (Get-Date).AddSeconds(45)
do {
  $running = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
    try {
      $path = $_.Path
      if (-not $path) { return $false }
      return ($roots | Where-Object { $path.StartsWith($_, [System.StringComparison]::OrdinalIgnoreCase) }).Count -gt 0
    } catch { return $false }
  })
  if ($running.Count -eq 0) { Start-Sleep -Milliseconds 500 }
} while ($running.Count -eq 0 -and (Get-Date) -lt $deadline)
if ($running.Count -eq 0) { throw 'ChatGPT/Codex desktop did not start within 45 seconds' }
[pscustomobject]@{ running = $true; processCount = $running.Count; executable = $entry[0].executable; codexHome = $codexHome } | ConvertTo-Json -Compress
"#;

    pub fn stop_for_codex_home_switch() -> Result<DesktopSwitch> {
        let output = run_powershell(STOP_SCRIPT, None)?;
        let result: StopResult = crate::json_compat::from_slice(&output)
            .context("解析 ChatGPT/Codex 桌面停止结果失败")?;
        Ok(DesktopSwitch {
            was_running: result.was_running,
        })
    }

    pub fn launch_and_verify(codex_home: &Path) -> Result<()> {
        run_powershell(LAUNCH_AND_VERIFY_SCRIPT, Some(codex_home))?;
        Ok(())
    }

    pub fn restart_and_verify(state: &DesktopSwitch, codex_home: &Path) -> Result<bool> {
        if !state.was_running {
            return Ok(false);
        }
        launch_and_verify(codex_home)?;
        Ok(true)
    }

    fn run_powershell(script: &str, codex_home: Option<&Path>) -> Result<Vec<u8>> {
        let mut command = Command::new("powershell.exe");
        crate::child_process::isolate_from_connector_environment(&mut command);
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ]);
        if let Some(codex_home) = codex_home {
            command.env("CODEX_HOME", codex_home);
        }
        let output = command
            .output()
            .context("启动 PowerShell 管理 ChatGPT/Codex 桌面进程失败")?;
        if !output.status.success() {
            anyhow::bail!(
                "管理 ChatGPT/Codex 桌面进程失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output.stdout)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Write;
        use std::process::Stdio;

        #[test]
        fn desktop_management_scripts_parse_in_windows_powershell() {
            for script in [STOP_SCRIPT, LAUNCH_AND_VERIFY_SCRIPT] {
                let mut child = Command::new("powershell.exe")
                    .args([
                        "-NoLogo",
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        "[scriptblock]::Create([Console]::In.ReadToEnd()) | Out-Null",
                    ])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(script.as_bytes())
                    .unwrap();
                let output = child.wait_with_output().unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use anyhow::Context;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::thread;
    use std::time::{Duration, Instant};

    const APPLICATION_PATHS: [&str; 2] = ["/Applications/ChatGPT.app", "/Applications/Codex.app"];
    const LAUNCH_TIMEOUT: Duration = Duration::from_secs(45);
    const POLL_INTERVAL: Duration = Duration::from_millis(500);

    pub fn stop_for_codex_home_switch() -> Result<DesktopSwitch> {
        let Some(app_path) = installed_application_path() else {
            return Ok(DesktopSwitch::default());
        };
        let bundle_id = application_bundle_id(&app_path)?;
        let info = application_info(&bundle_id)?;
        if !has_running_process(&info) {
            return Ok(DesktopSwitch::default());
        }

        let script = format!("tell application id \"{bundle_id}\" to quit");
        run_checked(
            {
                let mut command = Command::new("/usr/bin/osascript");
                command.args(["-e", &script]);
                command
            },
            "退出 ChatGPT/Codex 桌面应用失败",
        )?;
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if !has_running_process(&application_info(&bundle_id)?) {
                return Ok(DesktopSwitch { was_running: true });
            }
            thread::sleep(POLL_INTERVAL);
        }
        anyhow::bail!("ChatGPT/Codex 桌面应用未在 15 秒内退出")
    }

    pub fn restart_and_verify(state: &DesktopSwitch, codex_home: &Path) -> Result<bool> {
        if !state.was_running {
            return Ok(false);
        }
        launch_and_verify(codex_home)?;
        Ok(true)
    }

    pub fn launch_and_verify(codex_home: &Path) -> Result<()> {
        let app_path =
            installed_application_path().context("没有找到已安装的 ChatGPT/Codex 桌面应用")?;
        let bundle_id = application_bundle_id(&app_path)?;

        run_checked(
            open_application_command(&app_path, codex_home),
            "打开 ChatGPT/Codex 桌面应用失败",
        )?;

        let started = Instant::now();
        while started.elapsed() < LAUNCH_TIMEOUT {
            let info = application_info(&bundle_id)?;
            if has_running_process(&info) {
                verify_application_codex_home(&info, codex_home)?;
                return Ok(());
            }
            thread::sleep(POLL_INTERVAL);
        }

        anyhow::bail!("ChatGPT/Codex 桌面应用未在 45 秒内启动");
    }

    fn installed_application_path() -> Option<PathBuf> {
        APPLICATION_PATHS
            .iter()
            .map(PathBuf::from)
            .find(|path| path.is_dir())
    }

    fn application_bundle_id(app_path: &Path) -> Result<String> {
        let plist = app_path.join("Contents/Info.plist");
        let output = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Print :CFBundleIdentifier"])
            .arg(&plist)
            .output()
            .with_context(|| format!("读取桌面应用标识失败: {}", plist.display()))?;
        if !output.status.success() {
            anyhow::bail!("读取桌面应用标识失败：{}", command_error(&output));
        }
        let bundle_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if bundle_id.is_empty() {
            anyhow::bail!("桌面应用标识为空: {}", plist.display());
        }
        Ok(bundle_id)
    }

    fn application_info(bundle_id: &str) -> Result<String> {
        let output = Command::new("/usr/bin/lsappinfo")
            .args(["info", "-only", "pid", bundle_id])
            .output()
            .context("检查 ChatGPT/Codex 桌面进程失败")?;
        let mut info = String::from_utf8_lossy(&output.stdout).into_owned();
        info.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok(info)
    }

    fn has_running_process(info: &str) -> bool {
        info.lines().any(|line| {
            let line = line.trim();
            line.starts_with("\"pid\"=") && !line.contains("[ NULL ]")
        })
    }

    fn application_pid(info: &str) -> Option<u32> {
        info.lines().find_map(|line| {
            let line = line.trim();
            let value = line.strip_prefix("\"pid\"=")?.trim();
            value.parse().ok()
        })
    }

    fn verify_application_codex_home(info: &str, codex_home: &Path) -> Result<()> {
        let pid = application_pid(info).context("无法读取 ChatGPT/Codex 桌面进程 PID")?;
        let output = Command::new("/bin/ps")
            .args(["eww", "-p", &pid.to_string()])
            .output()
            .context("读取 ChatGPT/Codex 桌面进程环境失败")?;
        if !output.status.success() {
            anyhow::bail!(
                "读取 ChatGPT/Codex 桌面进程环境失败：{}",
                command_error(&output)
            );
        }
        let process = String::from_utf8_lossy(&output.stdout);
        let expected = format!("CODEX_HOME={}", codex_home.to_string_lossy());
        if !process.contains(&expected) {
            anyhow::bail!("ChatGPT/Codex 已启动，但没有使用所选工作区状态目录");
        }
        Ok(())
    }

    fn open_application_command(app_path: &Path, codex_home: &Path) -> Command {
        let mut command = Command::new("/usr/bin/open");
        crate::child_process::isolate_from_connector_environment(&mut command);
        let mut assignment = std::ffi::OsString::from("CODEX_HOME=");
        assignment.push(codex_home);
        command.arg("--env").arg(assignment);
        command.arg(app_path);
        command
    }

    fn run_checked(mut command: Command, context: &str) -> Result<()> {
        let output = command.output().with_context(|| context.to_string())?;
        if !output.status.success() {
            anyhow::bail!("{context}：{}", command_error(&output));
        }
        Ok(())
    }

    fn command_error(output: &Output) -> String {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            format!("exit={}", output.status)
        } else {
            stderr
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_lsappinfo_process_state_without_requiring_a_window() {
            let hidden = "\"pid\"=682\n\"visible\"=[ NULL ]\n\"windows\"=[ NULL ]\n";
            assert!(has_running_process(hidden));
            assert_eq!(application_pid(hidden), Some(682));

            let missing = "Application not found\n";
            assert!(!has_running_process(missing));
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::*;

    pub fn stop_for_codex_home_switch() -> Result<DesktopSwitch> {
        Ok(DesktopSwitch::default())
    }

    pub fn restart_and_verify(_state: &DesktopSwitch, _codex_home: &Path) -> Result<bool> {
        Ok(false)
    }
}
