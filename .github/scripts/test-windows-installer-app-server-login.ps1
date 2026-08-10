param(
  [Parameter(Mandatory = $true)]
  [string]$ScriptPath
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $ScriptPath -PathType Leaf)) {
  throw "Installer script was not found: $ScriptPath"
}

$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
  $ScriptPath,
  [ref]$tokens,
  [ref]$parseErrors
)
if ($parseErrors.Count -gt 0) {
  throw "Installer script has PowerShell parse errors: $($parseErrors[0].Message)"
}

$requiredFunctions = @(
  "Set-CodexProcessCommand",
  "Stop-CodexProcess",
  "Invoke-AppServerLogin"
)
foreach ($name in $requiredFunctions) {
  $definition = $ast.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
      $node.Name -eq $name
  }, $true) | Select-Object -First 1
  if (-not $definition) {
    throw "Installer script is missing function $name"
  }
  Invoke-Expression $definition.Extent.Text
}

$script:TestApiKey = 'lcmk_TEST_SECRET_123'
$script:Warnings = @()
function Get-CodexRouterApiKey { return $script:TestApiKey }
function Set-InstallStep([int]$index, [string]$state, [string]$detail) {}
function Add-Warning([string]$message) { $script:Warnings += $message }
function Reset-TestResult {
  $script:result = [ordered]@{
    codexExe = $null
    appServerLoginResponse = $false
    appServerLogin = $false
    appServerAccountType = $null
    appServerAuthModeUpdated = $false
  }
  $script:Warnings = @()
}

$testRoot = Join-Path $env:RUNNER_TEMP "codex-installer-app-server-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
$fakeServerPath = Join-Path $testRoot "fake-codex-app-server.ps1"
$fakeLauncherPath = Join-Path $testRoot "codex.cmd"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

$fakeServer = @'
$ErrorActionPreference = "Stop"

function Send-Message([object]$message) {
  [Console]::Out.WriteLine(($message | ConvertTo-Json -Compress -Depth 8))
  [Console]::Out.Flush()
}

while ($null -ne ($line = [Console]::In.ReadLine())) {
  try {
    $request = $line | ConvertFrom-Json -ErrorAction Stop
  } catch {
    $lineBytes = [System.Text.Encoding]::UTF8.GetBytes($line)
    [Console]::Error.WriteLine("FAKE_SERVER_INPUT_BASE64=$([Convert]::ToBase64String($lineBytes))")
    [Console]::Error.WriteLine($_.Exception.ToString())
    exit 90
  }
  if ($request.method -eq "initialize") {
    [Console]::Out.WriteLine("diagnostic line before initialize response")
    [Console]::Out.Flush()
    Send-Message ([ordered]@{ id = $request.id; result = [ordered]@{ userAgent = "fake" } })
    continue
  }
  if ($request.method -eq "initialized") { continue }
  if ($request.method -eq "account/login/start") {
    Send-Message ([ordered]@{ id = $request.id; result = [ordered]@{ type = "apiKey" } })
    if ($env:BAIJIMU_FAKE_LOGIN_SCENARIO -eq "rejected") {
      Send-Message ([ordered]@{
        method = "account/login/completed"
        params = [ordered]@{
          loginId = $null
          success = $false
          error = "denied by fake server for $($request.params.apiKey)"
        }
      })
      continue
    }
    Start-Sleep -Seconds 3
    Send-Message ([ordered]@{
      method = "account/login/completed"
      params = [ordered]@{ loginId = $null; success = $true; error = $null }
    })
    Send-Message ([ordered]@{
      method = "account/updated"
      params = [ordered]@{ authMode = "apikey"; planType = $null }
    })
    continue
  }
  if ($request.method -eq "account/read") {
    Send-Message ([ordered]@{
      id = $request.id
      result = [ordered]@{
        account = [ordered]@{ type = "apiKey" }
        requiresOpenaiAuth = $true
      }
    })
  }
}
'@

$fakeLauncher = @'
@echo off
powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "%~dp0fake-codex-app-server.ps1"
'@

[System.IO.File]::WriteAllText($fakeServerPath, $fakeServer, $utf8NoBom)
[System.IO.File]::WriteAllText($fakeLauncherPath, $fakeLauncher, $utf8NoBom)

try {
  Reset-TestResult
  $env:BAIJIMU_FAKE_LOGIN_SCENARIO = "delayed-success"
  Invoke-AppServerLogin $fakeLauncherPath
  if (-not $script:result.appServerLoginResponse) { throw "Login response was not recorded" }
  if (-not $script:result.appServerLogin) { throw "Login completion was not recorded" }
  if (-not $script:result.appServerAuthModeUpdated) { throw "Auth mode update was not recorded" }
  if ($script:result.appServerAccountType -ne "apiKey") { throw "Final account type was not verified" }
  if ($script:Warnings.Count -ne 1 -or $script:Warnings[0] -notmatch "non-JSON") {
    throw "Non-JSON app-server diagnostics were not handled as expected"
  }

  Reset-TestResult
  $env:BAIJIMU_FAKE_LOGIN_SCENARIO = "rejected"
  $rejection = $null
  try {
    Invoke-AppServerLogin $fakeLauncherPath
  } catch {
    $rejection = $_.Exception.Message
  }
  if (-not $rejection) { throw "Rejected login unexpectedly succeeded" }
  if ($rejection -notmatch "denied by fake server") { throw "Rejected login lost its actionable error: $rejection" }
  if ($rejection.Contains($script:TestApiKey)) { throw "Rejected login exposed the API key" }
  if ($rejection -notmatch '\*\*\*') { throw "Rejected login did not retain a masked credential marker" }

  Write-Host "Windows installer app-server login state machine verified"
} finally {
  Remove-Item Env:BAIJIMU_FAKE_LOGIN_SCENARIO -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
