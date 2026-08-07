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

    pub fn stop_for_codex_home_switch() -> Result<DesktopSwitch> {
        let output = run_powershell(STOP_SCRIPT, None)?;
        let result: StopResult =
            serde_json::from_slice(&output).context("解析 ChatGPT/Codex 桌面停止结果失败")?;
        Ok(DesktopSwitch {
            was_running: result.was_running,
            app_id: result.app_id,
        })
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

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub fn stop_for_codex_home_switch() -> Result<DesktopSwitch> {
        Ok(DesktopSwitch::default())
    }

    pub fn restart_and_verify(_state: &DesktopSwitch) -> Result<bool> {
        Ok(false)
    }
}
