use anyhow::Result;

#[derive(Clone, Debug, Default)]
pub struct DesktopSwitch {
    #[cfg(windows)]
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
    Start-Sleep -Milliseconds 250
    $remaining = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $targets.Id -contains $_.Id })
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
  Start-Sleep -Milliseconds 500
  $running = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
    try {
      $path = $_.Path
      if (-not $path) { return $false }
      return ($roots | Where-Object { $path.StartsWith($_, [System.StringComparison]::OrdinalIgnoreCase) }).Count -gt 0
    } catch { return $false }
  })
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
    Start-Sleep -Milliseconds 250
    $remaining = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $existing.Id -contains $_.Id })
  } while ($remaining.Count -gt 0 -and (Get-Date) -lt $deadline)
  if ($remaining.Count -gt 0) { throw 'ChatGPT/Codex desktop processes did not stop within 15 seconds' }
}
Start-Process explorer.exe "shell:AppsFolder\$($app[0].AppID)"
$deadline = (Get-Date).AddSeconds(45)
do {
  Start-Sleep -Milliseconds 500
  $running = @(Get-Process -ErrorAction SilentlyContinue | Where-Object {
    try {
      $path = $_.Path
      if (-not $path) { return $false }
      return ($roots | Where-Object { $path.StartsWith($_, [System.StringComparison]::OrdinalIgnoreCase) }).Count -gt 0
    } catch { return $false }
  })
  $visible = @($running | Where-Object { $_.MainWindowHandle -ne 0 })
} while ($visible.Count -eq 0 -and (Get-Date) -lt $deadline)
if ($visible.Count -eq 0) { throw 'ChatGPT/Codex desktop started but no visible window was detected within 45 seconds' }
[pscustomobject]@{ running = $true; processCount = $running.Count; visibleWindowCount = $visible.Count; appId = $app[0].AppID } | ConvertTo-Json -Compress
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
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::thread;
    use std::time::{Duration, Instant};

    const APPLICATION_PATHS: [&str; 2] = ["/Applications/ChatGPT.app", "/Applications/Codex.app"];
    const LAUNCH_TIMEOUT: Duration = Duration::from_secs(45);
    const PROJECT_REOPEN_DELAY: Duration = Duration::from_secs(6);
    const POLL_INTERVAL: Duration = Duration::from_millis(500);

    pub fn stop_for_codex_home_switch() -> Result<DesktopSwitch> {
        Ok(DesktopSwitch::default())
    }

    pub fn restart_and_verify(_state: &DesktopSwitch) -> Result<bool> {
        Ok(false)
    }

    pub fn launch_and_verify() -> Result<()> {
        let app_path =
            installed_application_path().context("没有找到已安装的 ChatGPT/Codex 桌面应用")?;
        let bundle_id = application_bundle_id(&app_path)?;

        run_checked(
            open_application_command(&app_path, None),
            "打开 ChatGPT/Codex 桌面应用失败",
        )?;

        let started = Instant::now();
        let mut project_reopened = false;
        let mut last_info = String::new();
        while started.elapsed() < LAUNCH_TIMEOUT {
            thread::sleep(POLL_INTERVAL);
            last_info = application_info(&bundle_id)?;
            if has_visible_window(&last_info) {
                return Ok(());
            }
            if !project_reopened && started.elapsed() >= PROJECT_REOPEN_DELAY {
                reopen_with_project(&app_path)?;
                project_reopened = true;
            }
        }

        if !has_running_process(&last_info) {
            anyhow::bail!("ChatGPT/Codex 桌面应用未在 45 秒内启动");
        }
        anyhow::bail!("ChatGPT/Codex 桌面应用已启动，但 45 秒内没有检测到可见窗口");
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
            .args(["info", "-only", "pid,front,visible,windows", bundle_id])
            .output()
            .context("检查 ChatGPT/Codex 桌面进程和窗口失败")?;
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

    fn has_visible_window(info: &str) -> bool {
        let application_visible = info.lines().any(|line| {
            let line = line.trim();
            line.starts_with("\"visible\"=")
                && !line.contains("[ NULL ]")
                && !line.ends_with("=false")
        });
        let window_present = info.lines().any(|line| {
            let line = line.trim();
            line.starts_with("\"windows\"=") && !line.contains("[ NULL ]")
        });
        application_visible && window_present
    }

    fn reopen_with_project(app_path: &Path) -> Result<()> {
        let home = env::var_os("HOME").context("HOME 未设置，无法创建 Codex 默认项目目录")?;
        let project = PathBuf::from(home)
            .join("Documents")
            .join("Codex")
            .join("default");
        fs::create_dir_all(&project)
            .with_context(|| format!("创建 Codex 默认项目目录失败: {}", project.display()))?;
        run_checked(
            open_application_command(app_path, Some(&project)),
            "请求 ChatGPT/Codex 打开默认项目失败",
        )
    }

    fn open_application_command(app_path: &Path, document: Option<&Path>) -> Command {
        let mut command = Command::new("/usr/bin/open");
        if let Some(codex_home) = env::var_os("CODEX_HOME") {
            let mut assignment = std::ffi::OsString::from("CODEX_HOME=");
            assignment.push(codex_home);
            command.arg("--env").arg(assignment);
        }
        if let Some(document) = document {
            command.arg("-a").arg(app_path).arg(document);
        } else {
            command.arg(app_path);
        }
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
        fn parses_lsappinfo_process_and_window_state() {
            let hidden = "\"pid\"=682\n\"visible\"=[ NULL ]\n\"windows\"=[ NULL ]\n";
            assert!(has_running_process(hidden));
            assert!(!has_visible_window(hidden));

            let hidden_with_window =
                "\"pid\"=682\n\"visible\"=false\n\"windows\"=( { \"windowID\"=123 } )\n";
            assert!(has_running_process(hidden_with_window));
            assert!(!has_visible_window(hidden_with_window));

            let visible = "\"pid\"=682\n\"visible\"=true\n\"windows\"=( { \"windowID\"=123 } )\n";
            assert!(has_running_process(visible));
            assert!(has_visible_window(visible));

            let missing = "Application not found\n";
            assert!(!has_running_process(missing));
            assert!(!has_visible_window(missing));
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
