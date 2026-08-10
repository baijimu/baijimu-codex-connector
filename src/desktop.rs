use anyhow::Result;

#[derive(Clone, Debug, Default)]
pub struct DesktopSwitch {
    #[cfg(any(windows, target_os = "macos"))]
    was_running: bool,
    #[cfg(windows)]
    app_id: Option<String>,
}

pub fn stop_for_codex_home_switch() -> Result<DesktopSwitch> {
    platform::stop_for_codex_home_switch()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn launch_and_verify() -> Result<()> {
    platform::launch_and_verify()
}

impl DesktopSwitch {
    pub fn restart_and_verify(&self) -> Result<bool> {
        platform::restart_and_verify(self)
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
        app_id: Option<String>,
    }

    const STOP_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$packages = @('OpenAI.Codex', 'OpenAI.ChatGPT') | ForEach-Object { Get-AppxPackage -Name $_ -ErrorAction SilentlyContinue } | Where-Object { $_ }
if (-not $packages) {
  $packages = @(Get-AppxPackage -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'OpenAI.Codex*' -or ($_.Name -like 'OpenAI.ChatGPT*' -and $_.Name -notlike 'OpenAI.ChatGPT-Desktop*') })
}
$roots = @($packages | ForEach-Object { $_.InstallLocation } | Where-Object { $_ })
$familyNames = @($packages | ForEach-Object { $_.PackageFamilyName } | Where-Object { $_ })
$app = @(Get-StartApps | Where-Object {
  $id = $_.AppID
  ($familyNames | Where-Object { $id -like "$_*" }).Count -gt 0
} | Select-Object -First 1)
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
[pscustomobject]@{ wasRunning = $wasRunning; appId = if ($app) { $app[0].AppID } else { $null } } | ConvertTo-Json -Compress
"#;

    const RESTART_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$appId = $env:BAIJIMU_CODEX_DESKTOP_APP_ID
if (-not $appId) { throw 'ChatGPT/Codex Start menu application id is unavailable' }
Start-Process explorer.exe "shell:AppsFolder\$appId"
$packages = @('OpenAI.Codex', 'OpenAI.ChatGPT') | ForEach-Object { Get-AppxPackage -Name $_ -ErrorAction SilentlyContinue } | Where-Object { $_ }
if (-not $packages) {
  $packages = @(Get-AppxPackage -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'OpenAI.Codex*' -or ($_.Name -like 'OpenAI.ChatGPT*' -and $_.Name -notlike 'OpenAI.ChatGPT-Desktop*') })
}
$roots = @($packages | ForEach-Object { $_.InstallLocation } | Where-Object { $_ })
$deadline = (Get-Date).AddSeconds(30)
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
if ($running.Count -eq 0) { throw 'ChatGPT/Codex desktop did not restart within 30 seconds' }
[pscustomobject]@{ running = $true; processCount = $running.Count } | ConvertTo-Json -Compress
"#;

    const LAUNCH_AND_VERIFY_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$packages = @('OpenAI.Codex', 'OpenAI.ChatGPT') | ForEach-Object { Get-AppxPackage -Name $_ -ErrorAction SilentlyContinue } | Where-Object { $_ }
if (-not $packages) {
  $packages = @(Get-AppxPackage -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'OpenAI.Codex*' -or ($_.Name -like 'OpenAI.ChatGPT*' -and $_.Name -notlike 'OpenAI.ChatGPT-Desktop*') })
}
if (-not $packages) { throw 'ChatGPT/Codex desktop package is not installed for the current user' }
$roots = @($packages | ForEach-Object { $_.InstallLocation } | Where-Object { $_ })
$familyNames = @($packages | ForEach-Object { $_.PackageFamilyName } | Where-Object { $_ })
$app = @(Get-StartApps | Where-Object {
  $id = $_.AppID
  ($familyNames | Where-Object { $id -like "$_*" }).Count -gt 0
} | Select-Object -First 1)
if (-not $app) { throw 'ChatGPT/Codex Start menu application id is unavailable' }
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
Start-Process explorer.exe "shell:AppsFolder\$($app[0].AppID)"
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
[pscustomobject]@{ running = $true; processCount = $running.Count; appId = $app[0].AppID } | ConvertTo-Json -Compress
"#;

    pub fn stop_for_codex_home_switch() -> Result<DesktopSwitch> {
        let output = run_powershell(STOP_SCRIPT, None)?;
        let result: StopResult = crate::json_compat::from_slice(&output)
            .context("解析 ChatGPT/Codex 桌面停止结果失败")?;
        Ok(DesktopSwitch {
            was_running: result.was_running,
            app_id: result.app_id,
        })
    }

    pub fn launch_and_verify() -> Result<()> {
        run_powershell(LAUNCH_AND_VERIFY_SCRIPT, None)?;
        Ok(())
    }

    pub fn restart_and_verify(state: &DesktopSwitch) -> Result<bool> {
        if !state.was_running {
            return Ok(false);
        }
        let app_id = state
            .app_id
            .as_deref()
            .context("切换前桌面应用正在运行，但没有找到 Start menu application id")?;
        run_powershell(RESTART_SCRIPT, Some(app_id))?;
        Ok(true)
    }

    fn run_powershell(script: &str, app_id: Option<&str>) -> Result<Vec<u8>> {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ]);
        if let Some(app_id) = app_id {
            command.env("BAIJIMU_CODEX_DESKTOP_APP_ID", app_id);
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
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use anyhow::Context;
    use std::env;
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

    pub fn restart_and_verify(state: &DesktopSwitch) -> Result<bool> {
        if !state.was_running {
            return Ok(false);
        }
        launch_and_verify()?;
        Ok(true)
    }

    pub fn launch_and_verify() -> Result<()> {
        let app_path =
            installed_application_path().context("没有找到已安装的 ChatGPT/Codex 桌面应用")?;
        let bundle_id = application_bundle_id(&app_path)?;

        run_checked(
            open_application_command(&app_path),
            "打开 ChatGPT/Codex 桌面应用失败",
        )?;

        let started = Instant::now();
        while started.elapsed() < LAUNCH_TIMEOUT {
            let info = application_info(&bundle_id)?;
            if has_running_process(&info) {
                verify_application_codex_home(&info)?;
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

    fn verify_application_codex_home(info: &str) -> Result<()> {
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
        match env::var_os("CODEX_HOME") {
            Some(expected) => {
                let expected = format!("CODEX_HOME={}", expected.to_string_lossy());
                if !process.contains(&expected) {
                    anyhow::bail!("ChatGPT/Codex 已启动，但没有使用所选工作区状态目录");
                }
            }
            None => {
                if process.contains("CODEX_HOME=") {
                    anyhow::bail!("ChatGPT/Codex 已启动，但仍继承了非预期的 CODEX_HOME");
                }
            }
        }
        Ok(())
    }

    fn open_application_command(app_path: &Path) -> Command {
        let mut command = Command::new("/usr/bin/open");
        if let Some(codex_home) = env::var_os("CODEX_HOME") {
            let mut assignment = std::ffi::OsString::from("CODEX_HOME=");
            assignment.push(codex_home);
            command.arg("--env").arg(assignment);
        }
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

    pub fn restart_and_verify(_state: &DesktopSwitch) -> Result<bool> {
        Ok(false)
    }
}
