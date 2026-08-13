$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$CodexModel = if ($env:CODEX_MODEL) { $env:CODEX_MODEL } else { "gpt-5.6-sol" }
if ($CodexModel -notmatch '^[A-Za-z0-9._-]+$') {
  throw "invalid CODEX_MODEL: $CodexModel"
}
$WorkspaceId = if ($env:CODEX_WORKSPACE_ID) { $env:CODEX_WORKSPACE_ID } elseif ($env:BAIJIMU_WORKSPACE_ID) { $env:BAIJIMU_WORKSPACE_ID } else { $env:WORKSPACE_ID }
$ProjectId = if ($env:CODEX_PROJECT_ID) { $env:CODEX_PROJECT_ID } elseif ($env:BAIJIMU_PROJECT_ID) { $env:BAIJIMU_PROJECT_ID } else { $env:PROJECT_ID }
$AgentConfigId = if ($env:CODEX_AGENT_CONFIG_ID) { $env:CODEX_AGENT_CONFIG_ID } else { $env:BAIJIMU_AGENT_CONFIG_ID }
$AgentSessionId = if ($env:CODEX_AGENT_SESSION_ID) { $env:CODEX_AGENT_SESSION_ID } else { $env:BAIJIMU_AGENT_SESSION_ID }
$SessionId = if ($env:CODEX_SESSION_ID) { $env:CODEX_SESSION_ID } elseif ($env:BAIJIMU_SESSION_ID) { $env:BAIJIMU_SESSION_ID } else { $env:SESSION_ID }
$RouterBaseUrl = if ($env:CODEX_ROUTER_BASE_URL) { $env:CODEX_ROUTER_BASE_URL.TrimEnd("/") } else { "https://router.baijimu.com/api/claudecode/v1" }
if (-not $WorkspaceId -or $WorkspaceId -notmatch '^\d+$') {
  throw "CODEX_WORKSPACE_ID or BAIJIMU_WORKSPACE_ID is required"
}
if ($ProjectId -and $ProjectId -notmatch '^\d+$') {
  throw "invalid CODEX_PROJECT_ID or BAIJIMU_PROJECT_ID"
}

$startedAt = Get-Date
$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
$codexDir = if ($env:CODEX_HOME) { $env:CODEX_HOME } else { Join-Path $env:USERPROFILE ".codex" }
$configPath = Join-Path $codexDir "config.toml"
$authPath = Join-Path $codexDir "auth.json"
$installStateDir = if ($env:CODEX_INSTALL_STATE_DIR) { $env:CODEX_INSTALL_STATE_DIR } else { Join-Path $env:TEMP "baijimu-codex-install" }
$statusPath = Join-Path $installStateDir "status.json"
$resultPath = Join-Path $installStateDir "result.json"
New-Item -ItemType Directory -Force -Path $installStateDir | Out-Null

$script:Utf8NoBomEncoding = New-Object System.Text.UTF8Encoding($false)

function Write-Utf8NoBomFile([string]$path, [AllowEmptyString()][string]$content) {
  $fullPath = [System.IO.Path]::GetFullPath($path)
  $directory = [System.IO.Path]::GetDirectoryName($fullPath)
  if (-not [string]::IsNullOrWhiteSpace($directory)) {
    [System.IO.Directory]::CreateDirectory($directory) | Out-Null
  }
  $temporaryPath = Join-Path $directory (".{0}.tmp-{1}-{2}" -f [System.IO.Path]::GetFileName($fullPath), $PID, [Guid]::NewGuid().ToString('N'))
  try {
    [System.IO.File]::WriteAllText($temporaryPath, $content, $script:Utf8NoBomEncoding)
    if (Test-Path -LiteralPath $fullPath -PathType Leaf) {
      [System.IO.File]::Replace(
        $temporaryPath,
        $fullPath,
        [System.Management.Automation.Language.NullString]::Value,
        $true
      )
    } else {
      [System.IO.File]::Move($temporaryPath, $fullPath)
    }
  } finally {
    if (Test-Path -LiteralPath $temporaryPath) {
      Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction SilentlyContinue
    }
  }
}

$script:CurrentStepIndex = 0
$script:InstallSteps = @(
  [pscustomobject]@{ index = 1; name = "Check ChatGPT desktop app"; state = "pending"; detail = ""; downloadedBytes = $null; totalBytes = $null },
  [pscustomobject]@{ index = 2; name = "Read package manifest"; state = "pending"; detail = ""; downloadedBytes = $null; totalBytes = $null },
  [pscustomobject]@{ index = 3; name = "Download ChatGPT package"; state = "pending"; detail = ""; downloadedBytes = $null; totalBytes = $null },
  [pscustomobject]@{ index = 4; name = "Verify package"; state = "pending"; detail = ""; downloadedBytes = $null; totalBytes = $null },
  [pscustomobject]@{ index = 5; name = "Install ChatGPT desktop app"; state = "pending"; detail = ""; downloadedBytes = $null; totalBytes = $null },
  [pscustomobject]@{ index = 6; name = "Create Baijimu LLM credential and config"; state = "pending"; detail = ""; downloadedBytes = $null; totalBytes = $null },
  [pscustomobject]@{ index = 7; name = "Verify Baijimu router"; state = "pending"; detail = ""; downloadedBytes = $null; totalBytes = $null },
  [pscustomobject]@{ index = 8; name = "Verify isolated Codex profile"; state = "pending"; detail = ""; downloadedBytes = $null; totalBytes = $null },
  [pscustomobject]@{ index = 9; name = "Verify Codex CLI"; state = "pending"; detail = ""; downloadedBytes = $null; totalBytes = $null },
  [pscustomobject]@{ index = 10; name = "Start and verify window"; state = "pending"; detail = ""; downloadedBytes = $null; totalBytes = $null }
)

$result = [ordered]@{
  ok = $false
  platform = "windows"
  startedAt = $startedAt.ToString("o")
  codexHome = $codexDir
  appInstalled = $false
  appInstallMethod = $null
  appId = $null
  codexExe = $null
  cliInstallMethod = $null
  cliArtifact = $null
  cliArtifactSha256 = $null
  workspaceId = [int64]$WorkspaceId
  projectId = if ($ProjectId) { [int64]$ProjectId } else { $null }
  baijimuCli = $null
  sharedCliTokenRead = $false
  llmCredentialCreated = $false
  codexAuthWritten = $false
  configWritten = $false
  authWritten = $false
  routerHttpStatus = $null
  appServerLoginResponse = $false
  appServerLogin = $false
  appServerAccountType = $null
  appServerAuthModeUpdated = $false
  cliVersion = $null
  cliSmoke = $false
  appStarted = $false
  visibleWindow = $false
  processCount = 0
  elapsedMs = 0
  model = $CodexModel
  warnings = @()
  errors = @()
}

function Add-Warning([string]$message) {
  $script:result.warnings += $message
}

function Add-Error([string]$message) {
  $script:result.errors += $message
}

function Write-InstallConsole([string]$message) {
  if ($env:CODEX_INSTALL_QUIET -eq "1") { return }
  [Console]::Error.WriteLine($message)
}

function Write-InstallStatus {
  $status = [ordered]@{
    title = "Baijimu is installing ChatGPT desktop app"
    platform = "windows"
    startedAt = $startedAt.ToString("o")
    updatedAt = (Get-Date).ToString("o")
    currentStep = $script:CurrentStepIndex
    statusPath = $statusPath
    resultPath = $resultPath
    steps = $script:InstallSteps
  }
  Write-Utf8NoBomFile $statusPath (($status | ConvertTo-Json -Depth 8) + "`n")
}

function Set-InstallStep([int]$index, [string]$state, [string]$detail = "", [Nullable[Int64]]$downloadedBytes = $null, [Nullable[Int64]]$totalBytes = $null) {
  $step = $script:InstallSteps | Where-Object { $_.index -eq $index } | Select-Object -First 1
  if (-not $step) { return }
  $script:CurrentStepIndex = $index
  $step.state = $state
  $step.detail = $detail
  $step.downloadedBytes = $downloadedBytes
  $step.totalBytes = $totalBytes
  Write-InstallStatus

  $label = "[{0}/{1}] {2}" -f $index, $script:InstallSteps.Count, $step.name
  if ($downloadedBytes -ne $null -and $totalBytes -ne $null -and $totalBytes -gt 0) {
    $downloadedMb = [math]::Round($downloadedBytes / 1MB, 1)
    $totalMb = [math]::Round($totalBytes / 1MB, 1)
    Write-InstallConsole ("{0}  {1}  {2}MB / {3}MB" -f $label, $state, $downloadedMb, $totalMb)
  } elseif ($detail) {
    Write-InstallConsole ("{0}  {1}  {2}" -f $label, $state, $detail)
  } else {
    Write-InstallConsole ("{0}  {1}" -f $label, $state)
  }
}

function Complete-PendingInstallSteps([string]$state, [string]$detail) {
  foreach ($step in $script:InstallSteps) {
    if ($step.state -eq "pending") {
      $step.state = $state
      $step.detail = $detail
    }
  }
  Write-InstallStatus
}

function Save-WebFileWithProgress([string]$uri, [string]$outFile, [int]$stepIndex, [string]$label, [Int64]$totalBytesHint = 0) {
  Set-InstallStep $stepIndex "running" $label
  $request = [System.Net.HttpWebRequest]::Create($uri)
  $request.UserAgent = "Baijimu-ChatGPT-Desktop-Installer/1.0"
  $request.Timeout = 1200000
  $request.ReadWriteTimeout = 1200000
  $response = $request.GetResponse()
  try {
    $totalBytes = [Int64]$response.ContentLength
    if ($totalBytesHint -gt 0) { $totalBytes = $totalBytesHint }
    $inputStream = $response.GetResponseStream()
    $outputStream = [System.IO.File]::Open($outFile, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
    try {
      $buffer = New-Object byte[] 1048576
      [Int64]$downloadedBytes = 0
      $lastUpdate = Get-Date
      while (($read = $inputStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
        $outputStream.Write($buffer, 0, $read)
        $downloadedBytes += $read
        if (((Get-Date) - $lastUpdate).TotalSeconds -ge 1) {
          Set-InstallStep $stepIndex "running" $label $downloadedBytes $totalBytes
          $lastUpdate = Get-Date
        }
      }
      Set-InstallStep $stepIndex "completed" $label $downloadedBytes $totalBytes
    } finally {
      $outputStream.Close()
      $inputStream.Close()
    }
  } finally {
    $response.Close()
  }
}

Write-InstallConsole ""
Write-InstallConsole "Baijimu is installing ChatGPT desktop app"
Write-InstallConsole "Please keep this window open."
Write-InstallConsole ""
Write-InstallStatus

function Get-CodexStartApp {
  $apps = @(Get-StartApps)
  $package = Get-CodexInstalledPackage
  if ($package -and $package.PackageFamilyName) {
    $matched = $apps | Where-Object { $_.AppID -like "$($package.PackageFamilyName)*" } | Select-Object -First 1
    if ($matched) { return $matched }
  }
  $apps |
    Where-Object {
      $_.Name -like "*Codex*" -or
      $_.AppID -like "OpenAI.Codex*" -or
      $_.AppID -like "OpenAI.ChatGPT_*"
    } |
    Select-Object -First 1
}

function Get-CodexInstalledPackage {
  $packageNames = @("OpenAI.Codex", "OpenAI.ChatGPT")
  foreach ($packageName in $packageNames) {
    $package = Get-AppxPackage -Name $packageName -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($package) { return $package }
  }
  Get-AppxPackage -ErrorAction SilentlyContinue |
    Where-Object {
      $_.Name -like "OpenAI.Codex*" -or
      ($_.Name -like "OpenAI.ChatGPT*" -and $_.Name -notlike "OpenAI.ChatGPT-Desktop*")
    } |
    Select-Object -First 1
}

function Get-CodexWindowsAppAssetName {
  $arch = (Get-CimInstance Win32_Processor | Select-Object -First 1).Architecture
  if ($arch -eq 12 -or $env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
    return "codex-app-windows-arm64.msix"
  }
  return "codex-app-windows-x64.msix"
}

function Wait-CodexStartApp([int]$timeoutSeconds) {
  $deadline = (Get-Date).AddSeconds($timeoutSeconds)
  do {
    $app = Get-CodexStartApp
    if ($app) { return $app }
    Start-Sleep -Seconds 2
  } while ((Get-Date) -lt $deadline)
  return $null
}

function Get-CodexCacheAsset([string]$assetName) {
  Set-InstallStep 2 "running" "Reading Baijimu package manifest"
  $manifestUrl = "https://download.baijimu.com/codex-artifacts/latest.json"
  $manifestPath = Join-Path $env:TEMP "codex-artifacts-latest.json"
  Save-WebFileWithProgress $manifestUrl $manifestPath 2 "Reading Baijimu package manifest"
  $manifest = Get-Content -Raw -Path $manifestPath | ConvertFrom-Json
  $asset = @($manifest.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1)
  if (-not $asset) {
    throw "baijimu cache missing asset: $assetName"
  }
  if (-not $asset.mirror_url -or -not $asset.sha256) {
    throw "baijimu cache asset is incomplete: $assetName"
  }
  Set-InstallStep 2 "completed" "Found $assetName"
  return $asset
}

function Install-CodexAppFromBaijimuCache {
  $assetName = Get-CodexWindowsAppAssetName
  $asset = Get-CodexCacheAsset $assetName
  $packagePath = Join-Path $env:TEMP $assetName
  $assetSize = 0
  if ($asset.size_bytes) { $assetSize = [Int64]$asset.size_bytes }
  elseif ($asset.size) { $assetSize = [Int64]$asset.size }
  elseif ($asset.file_size) { $assetSize = [Int64]$asset.file_size }
  Save-WebFileWithProgress $asset.mirror_url $packagePath 3 "Downloading official ChatGPT desktop app package" $assetSize
  Set-InstallStep 4 "running" "Verifying package SHA256"
  $actual = (Get-FileHash -Algorithm SHA256 -Path $packagePath).Hash.ToLowerInvariant()
  $expected = [string]$asset.sha256
  if ($actual -ne $expected.ToLowerInvariant()) {
    throw "SHA256 mismatch for $assetName"
  }
  Set-InstallStep 4 "completed" "Package SHA256 verified"
  Unblock-File -Path $packagePath -ErrorAction SilentlyContinue
  $script:result.appInstallMethod = "baijimu-cache-msix"
  Set-InstallStep 5 "running" "Installing ChatGPT desktop app"
  Add-AppxPackage -Path $packagePath
  Set-InstallStep 5 "completed" "ChatGPT desktop app installed"
}

function Ensure-CodexApp {
  Set-InstallStep 1 "running" "Checking whether ChatGPT desktop app is installed"
  $app = Get-CodexStartApp
  if ($app) {
    $script:result.appInstalled = $true
    $script:result.appInstallMethod = "already-installed"
    $script:result.appId = $app.AppID
    Set-InstallStep 1 "completed" "ChatGPT desktop app is already installed"
    Set-InstallStep 2 "skipped" "Package download is not needed"
    Set-InstallStep 3 "skipped" "Package download is not needed"
    Set-InstallStep 4 "skipped" "Package verification is not needed"
    Set-InstallStep 5 "skipped" "Reinstall is not needed"
    return
  }

  $package = Get-CodexInstalledPackage
  if ($package) {
    $script:result.appInstalled = $true
    $script:result.appInstallMethod = "already-installed"
    $app = Wait-CodexStartApp 20
    if ($app) { $script:result.appId = $app.AppID }
    Set-InstallStep 1 "completed" "ChatGPT desktop app package is already installed"
    Set-InstallStep 2 "skipped" "Package download is not needed"
    Set-InstallStep 3 "skipped" "Package download is not needed"
    Set-InstallStep 4 "skipped" "Package verification is not needed"
    Set-InstallStep 5 "skipped" "Reinstall is not needed"
    return
  }
  Set-InstallStep 1 "completed" "ChatGPT desktop app is not installed; preparing install"

  try {
    Install-CodexAppFromBaijimuCache
  } catch {
    Add-Warning "baijimu cache install failed: $($_.Exception.Message)"
    if ($env:CODEX_ALLOW_OFFICIAL_WINDOWS_INSTALLER_FALLBACK -eq "1") {
      $script:result.appInstallMethod = "official-installer"
      $installer = Join-Path $env:TEMP "ChatGPT Installer.exe"
      Save-WebFileWithProgress "https://get.microsoft.com/installer/download/9PLM9XGG6VKS?cid=website_cta_psi" $installer 3 "Downloading official installer"
      Set-InstallStep 5 "running" "Running official installer"
      Start-Process -FilePath $installer -Wait
      Set-InstallStep 5 "completed" "Official installer completed"
    } elseif ($env:CODEX_ALLOW_WINGET_FALLBACK -eq "1") {
      $winget = Get-Command winget -ErrorAction SilentlyContinue
      if (-not $winget) {
        throw "baijimu cache install failed and winget is unavailable"
      }
      $script:result.appInstallMethod = "winget-msstore"
      Set-InstallStep 5 "running" "Installing through Microsoft Store"
      & winget install --id 9PLM9XGG6VKS -s msstore --accept-package-agreements --accept-source-agreements | Out-Null
      Set-InstallStep 5 "completed" "Microsoft Store install completed"
    } else {
      throw
    }
  }

  $app = Wait-CodexStartApp 60
  if (-not $app) {
    $package = Get-CodexInstalledPackage
    if ($package) {
      throw "ChatGPT desktop app package was installed but no Start menu entry was found after installation"
    }
    throw "ChatGPT desktop app package and Start menu entry were not found after installation"
  }

  $script:result.appInstalled = $true
  $script:result.appId = $app.AppID
  Set-InstallStep 5 "completed" "ChatGPT desktop app can start"
}

function Get-CodexRouterApiKey {
  if (-not (Test-Path $authPath)) {
    throw "Codex auth file was not written"
  }
  $auth = Get-Content -Raw -Path $authPath | ConvertFrom-Json
  if (-not $auth.OPENAI_API_KEY) {
    throw "Codex auth file does not contain OPENAI_API_KEY"
  }
  return [string]($auth.OPENAI_API_KEY)
}

function Resolve-BaijimuCli {
  if ($env:BAIJIMU_CLI_BIN -and (Test-Path $env:BAIJIMU_CLI_BIN)) {
    $script:result.baijimuCli = $env:BAIJIMU_CLI_BIN
    return $env:BAIJIMU_CLI_BIN
  }
  $command = Get-Command baijimu -ErrorAction SilentlyContinue
  if ($command -and $command.Source) {
    $script:result.baijimuCli = $command.Source
    return $command.Source
  }
  $candidates = @()
  if ($env:LOCALAPPDATA) {
    $candidates += (Join-Path $env:LOCALAPPDATA "Baijimu\bin\baijimu.exe")
  }
  if ($env:USERPROFILE) {
    $candidates += (Join-Path $env:USERPROFILE ".local\bin\baijimu.exe")
  }
  foreach ($candidate in $candidates) {
    if ($candidate -and (Test-Path $candidate)) {
      $script:result.baijimuCli = $candidate
      return $candidate
    }
  }
  throw "baijimu CLI was not found; please update or restart Baijimu Bridge Agent"
}

function New-BaijimuLlmCredential {
  if ($env:CODEX_LLM_CREDENTIAL_FILE) {
    if (-not (Test-Path -LiteralPath $env:CODEX_LLM_CREDENTIAL_FILE -PathType Leaf)) {
      throw "CODEX_LLM_CREDENTIAL_FILE does not exist"
    }
    $credential = (Get-Content -Raw -LiteralPath $env:CODEX_LLM_CREDENTIAL_FILE).Trim()
    if (-not $credential) {
      throw "CODEX_LLM_CREDENTIAL_FILE is empty"
    }
    $script:result.llmCredentialCreated = $true
    return [string]$credential
  }
  $baijimu = Resolve-BaijimuCli
  $outputPath = Join-Path $installStateDir "baijimu-llm-credential.json"
  $errorPath = Join-Path $installStateDir "baijimu-llm-credential.err"
  Remove-Item $outputPath, $errorPath -Force -ErrorAction SilentlyContinue

  $args = @(
    "--json",
    "llm-credential",
    "create",
    "--workspace-id",
    $WorkspaceId,
    "--show-secret"
  )
  if ($ProjectId) {
    $args += @("--project-id", $ProjectId)
  }
  if ($AgentConfigId) {
    $args += @("--agent-config-id", $AgentConfigId)
  }
  if ($AgentSessionId) {
    $args += @("--agent-session-id", $AgentSessionId)
  }
  if ($SessionId) {
    $args += @("--session-id", $SessionId)
  }

  $process = Start-Process -FilePath $baijimu -ArgumentList $args -NoNewWindow -Wait -PassThru -RedirectStandardOutput $outputPath -RedirectStandardError $errorPath
  if ($process.ExitCode -ne 0) {
    $errorText = if (Test-Path $errorPath) { (Get-Content -Raw -Path $errorPath).Trim() } else { "" }
    throw "baijimu llm-credential create failed: $errorText"
  }
  if (-not (Test-Path $outputPath)) {
    throw "baijimu llm-credential create did not produce output"
  }
  $payload = Get-Content -Raw -Path $outputPath | ConvertFrom-Json
  Remove-Item $outputPath -Force -ErrorAction SilentlyContinue
  $data = if ($payload.data) { $payload.data } else { $payload }
  $credential = if ($data.llmCredential) { $data.llmCredential } elseif ($data.credential) { $data.credential } else { $data.apiKey }
  if (-not $credential) {
    throw "baijimu llm-credential create did not return an LLM credential"
  }
  return [string]$credential
}

function Remove-ManagedCodexBlock([string]$content) {
  $lines = New-Object System.Collections.Generic.List[string]
  $skipping = $false
  foreach ($line in ($content -split "`r?`n")) {
    if ($line.Trim() -eq "# >>> baijimu managed codex router") {
      $skipping = $true
      continue
    }
    if ($skipping) {
      if ($line.Trim() -eq "# <<< baijimu managed codex router") {
        $skipping = $false
      }
      continue
    }
    $lines.Add($line)
  }
  return ($lines -join "`n")
}

function Remove-TomlTable([string]$content, [string]$tableName) {
  $lines = New-Object System.Collections.Generic.List[string]
  $skipping = $false
  foreach ($line in ($content -split "`r?`n")) {
    $trimmed = $line.Trim()
    if ($trimmed -eq $tableName) {
      $skipping = $true
      continue
    }
    if ($skipping -and $trimmed -match '^\[.+\]$') {
      $skipping = $false
    }
    if (-not $skipping) {
      $lines.Add($line)
    }
  }
  return ($lines -join "`n")
}

function ConvertTo-CodexConfigContent([string]$existing) {
  $content = Remove-ManagedCodexBlock $existing
  $content = Remove-TomlTable $content "[model_providers.baijimu-router]"
  foreach ($key in @("model_provider", "model", "sandbox_mode", "approval_policy", "cli_auth_credentials_store", "forced_login_method")) {
    $content = ($content -split "`r?`n" | Where-Object { $_ -notmatch "^\s*$([regex]::Escape($key))\s*=" }) -join "`n"
  }
  $managed = @(
    "# >>> baijimu managed codex router",
    'model_provider = "baijimu-router"',
    ('model = "{0}"' -f $CodexModel.Replace('\', '\\').Replace('"', '\"')),
    'sandbox_mode = "danger-full-access"',
    'approval_policy = "on-request"',
    'cli_auth_credentials_store = "file"',
    'forced_login_method = "api"',
    "",
    "[model_providers.baijimu-router]",
    'name = "baijimu-router"',
    ('base_url = "{0}"' -f $RouterBaseUrl.Replace('\', '\\').Replace('"', '\"')),
    'wire_api = "responses"',
    'requires_openai_auth = true',
    "# <<< baijimu managed codex router"
  ) -join "`n"
  $preserved = $content.Trim()
  if ($preserved) {
    return "$managed`n`n$preserved`n"
  }
  return "$managed`n"
}

function Backup-IfExists([string]$path) {
  if (Test-Path $path) {
    $suffix = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    Copy-Item $path "$path.bak.$suffix" -Force
  }
}

function Write-CodexConfig {
  Set-InstallStep 6 "running" "Creating Baijimu LLM credential and writing Codex config"
  New-Item -ItemType Directory -Force -Path $codexDir | Out-Null
  $cliToken = New-BaijimuLlmCredential
  $script:result.sharedCliTokenRead = $true
  $script:result.llmCredentialCreated = $true
  Backup-IfExists $authPath
  Backup-IfExists $configPath
  $authContent = [ordered]@{
    OPENAI_API_KEY = $cliToken
    auth_mode = "apikey"
  } | ConvertTo-Json -Depth 4
  Write-Utf8NoBomFile $authPath ($authContent + "`n")
  $existingConfig = if (Test-Path $configPath) { Get-Content -Raw -Path $configPath } else { "" }
  Write-Utf8NoBomFile $configPath (ConvertTo-CodexConfigContent $existingConfig)
  Remove-Variable cliToken -ErrorAction SilentlyContinue
  $script:result.codexAuthWritten = $true
  $script:result.configWritten = $true

  [void](Get-CodexRouterApiKey)
  $script:result.authWritten = $true
  Set-InstallStep 6 "completed" "Codex config written from Baijimu LLM credential"
}

function Test-Router {
  Set-InstallStep 7 "running" "Verifying Baijimu router"
  $apiKey = Get-CodexRouterApiKey
  $responsePath = Join-Path $env:TEMP "codex-router-responses.json"
  $headers = @{
    Authorization = "Bearer $apiKey"
    "Content-Type" = "application/json"
  }
  $body = @{
    model = $CodexModel
    input = "Reply with exactly OK"
  } | ConvertTo-Json -Compress
  try {
    $routerResponse = Invoke-WebRequest -UseBasicParsing -TimeoutSec 60 -Method POST -Uri "$RouterBaseUrl/responses" -Headers $headers -Body $body -OutFile $responsePath -PassThru
    $script:result.routerHttpStatus = [int]$routerResponse.StatusCode
  } finally {
    Remove-Item $responsePath -Force -ErrorAction SilentlyContinue
  }
  if ($script:result.routerHttpStatus -ne 200) {
    throw "router /responses health check failed: HTTP $($script:result.routerHttpStatus)"
  }
  Set-InstallStep 7 "completed" "Baijimu router verified"
}

function Start-CodexDesktop {
  if (-not $script:result.appId) {
    $app = Get-CodexStartApp
    if ($app) { $script:result.appId = $app.AppID }
  }
  if ($script:result.appId) {
    Start-Process explorer.exe "shell:AppsFolder\$($script:result.appId)"
    $script:result.appStarted = $true
  }
}

function Get-CodexProcesses {
  Get-Process -ErrorAction SilentlyContinue |
    Where-Object {
      $_.ProcessName -like "Codex*" -or
      $_.ProcessName -eq "codex" -or
      $_.ProcessName -eq "ChatGPT"
    }
}

function Get-CodexVisibleWindows {
  Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class BaijimuCodexWindowEnum {
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
}
"@ -ErrorAction SilentlyContinue

  $windows = New-Object System.Collections.Generic.List[object]
  $callback = [BaijimuCodexWindowEnum+EnumWindowsProc]{
    param([IntPtr]$hWnd, [IntPtr]$lParam)
    if ([BaijimuCodexWindowEnum]::IsWindowVisible($hWnd)) {
      $titleBuilder = New-Object System.Text.StringBuilder 512
      [void][BaijimuCodexWindowEnum]::GetWindowText($hWnd, $titleBuilder, $titleBuilder.Capacity)
      $title = $titleBuilder.ToString()
      [uint32]$windowProcessId = 0
      [void][BaijimuCodexWindowEnum]::GetWindowThreadProcessId($hWnd, [ref]$windowProcessId)
      $process = Get-Process -Id $windowProcessId -ErrorAction SilentlyContinue
      if ($process -and (
        $process.ProcessName -like "Codex*" -or
        $process.ProcessName -eq "codex" -or
        $process.ProcessName -eq "ChatGPT" -or
        ($process.ProcessName -eq "ApplicationFrameHost" -and $title -match '(Codex|ChatGPT)')
      )) {
        $windows.Add([pscustomobject]@{
          handle = $hWnd.ToInt64()
          processName = $process.ProcessName
          title = $title
        }) | Out-Null
      }
    }
    return $true
  }
  [void][BaijimuCodexWindowEnum]::EnumWindows($callback, [IntPtr]::Zero)
  return $windows
}

function Get-CodexCliAssetName {
  $arch = (Get-CimInstance Win32_Processor | Select-Object -First 1).Architecture
  if ($arch -eq 12 -or $env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
    return "codex-package-aarch64-pc-windows-msvc.tar.gz"
  }
  return "codex-package-x86_64-pc-windows-msvc.tar.gz"
}

function Get-CodexCliTarget {
  $arch = (Get-CimInstance Win32_Processor | Select-Object -First 1).Architecture
  if ($arch -eq 12 -or $env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
    return "aarch64-pc-windows-msvc"
  }
  return "x86_64-pc-windows-msvc"
}

function Get-ConfiguredCodexCli {
  if (-not $env:CODEX_CLI_BIN) { return $null }
  if (-not [System.IO.Path]::IsPathRooted($env:CODEX_CLI_BIN)) {
    throw "CODEX_CLI_BIN must be an absolute path"
  }
  if (-not (Test-Path -LiteralPath $env:CODEX_CLI_BIN -PathType Leaf)) {
    throw "CODEX_CLI_BIN does not exist: $($env:CODEX_CLI_BIN)"
  }
  $extension = [System.IO.Path]::GetExtension($env:CODEX_CLI_BIN)
  if ($extension -notin @(".exe", ".com", ".cmd", ".bat")) {
    throw "CODEX_CLI_BIN must point to a Windows executable or command launcher"
  }
  if (-not (Test-CodexCliCandidate $env:CODEX_CLI_BIN "CODEX_CLI_BIN")) {
    throw "CODEX_CLI_BIN does not point to a working Codex CLI with app-server support: $($env:CODEX_CLI_BIN)"
  }
  $script:result.cliInstallMethod = "advanced-absolute-path"
  return $env:CODEX_CLI_BIN
}

function Test-CodexCliCandidate([string]$codexExe, [string]$label) {
  $versionProbe = Invoke-CodexProcess $codexExe "--version" 20
  if ($versionProbe.timedOut -or $versionProbe.exitCode -ne 0) {
    Add-Warning "ignored unusable $label Codex CLI ($codexExe): codex --version failed: $($versionProbe.stderr)"
    return $false
  }
  $appServerProbe = Invoke-CodexProcess $codexExe "app-server --help" 20
  if ($appServerProbe.timedOut -or $appServerProbe.exitCode -ne 0) {
    Add-Warning "ignored unusable $label Codex CLI ($codexExe): codex app-server --help failed: $($appServerProbe.stderr)"
    return $false
  }
  return $true
}

function Get-SystemCodexCli {
  $commands = @(Get-Command codex -All -ErrorAction SilentlyContinue)
  $candidates = @($commands |
    Where-Object {
      $_.Source -and
      [System.IO.Path]::GetExtension($_.Source) -in @(".exe", ".com", ".cmd", ".bat") -and
      (Test-Path -LiteralPath $_.Source -PathType Leaf)
    })
  $seen = @{}
  foreach ($command in $candidates) {
    $candidate = [System.IO.Path]::GetFullPath([string]$command.Source)
    if ($seen.ContainsKey($candidate)) { continue }
    $seen[$candidate] = $true
    if (Test-CodexCliCandidate $candidate "system") {
      $script:result.cliInstallMethod = "already-installed"
      return $candidate
    }
  }
  return $null
}

function Read-ManagedCodexCli([string]$statePath) {
  if (-not (Test-Path -LiteralPath $statePath -PathType Leaf)) { return $null }
  try {
    $state = Get-Content -Raw -LiteralPath $statePath | ConvertFrom-Json
    if ($state.binaryPath -and (Test-Path -LiteralPath $state.binaryPath -PathType Leaf)) {
      return $state
    }
  } catch {
    Add-Warning "ignored invalid managed Codex CLI state: $($_.Exception.Message)"
  }
  return $null
}

function Resolve-CodexPackageContents([string]$packageDir, [string]$expectedTarget) {
  $packageRoot = [System.IO.Path]::GetFullPath($packageDir)
  $metadataPath = Join-Path $packageRoot "codex-package.json"
  if (-not (Test-Path -LiteralPath $metadataPath -PathType Leaf)) {
    throw "official Codex package metadata is missing: codex-package.json"
  }
  try {
    $metadata = Get-Content -Raw -LiteralPath $metadataPath | ConvertFrom-Json -ErrorAction Stop
  } catch {
    throw "official Codex package metadata is invalid: $($_.Exception.Message)"
  }
  if ($metadata.variant -ne "codex" -or $metadata.target -ne $expectedTarget) {
    throw "official Codex package identity mismatch: variant=$($metadata.variant), target=$($metadata.target)"
  }
  if ($metadata.entrypoint -ne "bin/codex.exe" -and $metadata.entrypoint -ne "bin\codex.exe") {
    throw "official Codex package entrypoint is unexpected: $($metadata.entrypoint)"
  }
  if ($metadata.resourcesDir -ne "codex-resources" -or $metadata.pathDir -ne "codex-path") {
    throw "official Codex package resource layout is unexpected"
  }

  $requiredFiles = @(
    [string]$metadata.entrypoint,
    "bin\codex-code-mode-host.exe",
    "$($metadata.resourcesDir)\codex-command-runner.exe",
    "$($metadata.resourcesDir)\codex-windows-sandbox-setup.exe",
    "$($metadata.pathDir)\rg.exe"
  )
  $resolvedFiles = [ordered]@{}
  $packagePrefix = $packageRoot.TrimEnd('\') + '\'
  foreach ($relativePath in $requiredFiles) {
    $candidate = [System.IO.Path]::GetFullPath((Join-Path $packageRoot $relativePath))
    if (-not $candidate.StartsWith($packagePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
      throw "official Codex package contains an unsafe path: $relativePath"
    }
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
      throw "official Codex package is incomplete: $relativePath"
    }
    $resolvedFiles[$relativePath] = $candidate
  }

  return [pscustomobject]@{
    metadata = $metadata
    binaryPath = [string]$resolvedFiles[[string]$metadata.entrypoint]
    packageRoot = $packageRoot
  }
}

function Remove-LegacyManagedCodexCli([object]$state, [string]$cliRoot, [string]$activePackageRoot) {
  if (-not $state -or -not ([string]$state.artifact).EndsWith(".exe.zip", [System.StringComparison]::OrdinalIgnoreCase)) {
    return
  }
  $legacyBinary = [System.IO.Path]::GetFullPath([string]$state.binaryPath)
  $legacyVersionDir = [System.IO.Path]::GetDirectoryName($legacyBinary)
  $versionsRoot = [System.IO.Path]::GetFullPath((Join-Path $cliRoot "versions"))
  if ([System.IO.Path]::GetDirectoryName($legacyVersionDir) -ne $versionsRoot) {
    Add-Warning "ignored legacy Codex CLI outside the managed versions directory: $legacyBinary"
    return
  }
  if ($legacyVersionDir -eq [System.IO.Path]::GetFullPath($activePackageRoot)) { return }
  Remove-Item -LiteralPath $legacyVersionDir -Recurse -Force -ErrorAction SilentlyContinue
}

function Install-CodexCliFromBaijimuCache {
  Set-InstallStep 8 "running" "Resolving the current official Codex CLI artifact"
  $assetName = Get-CodexCliAssetName
  $manifestUrl = "https://download.baijimu.com/codex-artifacts/latest.json"
  $manifest = Invoke-RestMethod -UseBasicParsing -TimeoutSec 120 -Uri $manifestUrl
  $asset = @($manifest.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1)
  if (-not $asset -or -not $asset.mirror_url -or -not $asset.sha256) {
    throw "baijimu cache missing complete Codex CLI asset: $assetName"
  }

  $expected = ([string]$asset.sha256).ToLowerInvariant()
  if ($asset.install_layout -ne "codex_package_v1") {
    throw "baijimu cache returned an unsupported Codex CLI install layout: $($asset.install_layout)"
  }
  $expectedTarget = Get-CodexCliTarget
  $cliRoot = Join-Path $env:LOCALAPPDATA "OpenAI\Codex\cli"
  $statePath = Join-Path $cliRoot "current.json"
  $current = Read-ManagedCodexCli $statePath
  if ($current -and $current.artifactSha256 -eq $expected -and $current.packageLayout -eq "codex_package_v1") {
    try {
      $installedPackage = Resolve-CodexPackageContents ([string]$current.packageRoot) $expectedTarget
      if ($installedPackage.binaryPath -ne [System.IO.Path]::GetFullPath([string]$current.binaryPath)) {
        throw "managed Codex CLI entrypoint does not match package metadata"
      }
      if (-not (Test-CodexCliCandidate $installedPackage.binaryPath "managed")) {
        throw "managed Codex CLI failed executable verification"
      }
      $script:result.cliInstallMethod = "managed-current"
      $script:result.cliArtifact = $assetName
      $script:result.cliArtifactSha256 = $expected
      return $installedPackage.binaryPath
    } catch {
      Add-Warning "managed Codex CLI package is incomplete and will be reinstalled: $($_.Exception.Message)"
    }
  }

  $temporaryRoot = Join-Path $env:TEMP "baijimu-codex-cli-$([Guid]::NewGuid().ToString('N'))"
  $downloadPath = Join-Path $temporaryRoot $assetName
  $versionDir = Join-Path $cliRoot (Join-Path "versions" $expected)
  $stagingDir = $null
  New-Item -ItemType Directory -Force -Path $temporaryRoot | Out-Null
  try {
    Save-WebFileWithProgress $asset.mirror_url $downloadPath 8 "Downloading official Codex CLI"
    Set-InstallStep 8 "running" "Verifying official Codex CLI SHA256"
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $downloadPath).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
      throw "SHA256 mismatch for $assetName"
    }

    $versionsRoot = Join-Path $cliRoot "versions"
    New-Item -ItemType Directory -Force -Path $versionsRoot | Out-Null
    $stagingDir = Join-Path $versionsRoot ".staging-$expected-$([Guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null
    $tar = Get-Command tar.exe -ErrorAction SilentlyContinue
    if (-not $tar) { throw "Windows tar.exe is required to install the official Codex package" }
    & $tar.Source -xzf $downloadPath -C $stagingDir
    if ($LASTEXITCODE -ne 0) { throw "failed to extract official Codex package: exit code $LASTEXITCODE" }
    $stagedPackage = Resolve-CodexPackageContents $stagingDir $expectedTarget

    $versionProbe = Invoke-CodexProcess $stagedPackage.binaryPath "--version" 20
    if ($versionProbe.timedOut -or $versionProbe.exitCode -ne 0) {
      throw "official Codex package failed version verification: $($versionProbe.stderr)"
    }
    $appServerProbe = Invoke-CodexProcess $stagedPackage.binaryPath "app-server --help" 20
    if ($appServerProbe.timedOut -or $appServerProbe.exitCode -ne 0) {
      throw "official Codex package failed app-server verification: $($appServerProbe.stderr)"
    }

    if (Test-Path -LiteralPath $versionDir) {
      try {
        $existingPackage = Resolve-CodexPackageContents $versionDir $expectedTarget
        if (-not (Test-CodexCliCandidate $existingPackage.binaryPath "managed")) {
          throw "existing managed Codex CLI failed executable verification"
        }
        Remove-Item -LiteralPath $stagingDir -Recurse -Force
      } catch {
        Remove-Item -LiteralPath $versionDir -Recurse -Force
        Move-Item -LiteralPath $stagingDir -Destination $versionDir
      }
    } else {
      Move-Item -LiteralPath $stagingDir -Destination $versionDir
    }
  } finally {
    Remove-Item -LiteralPath $temporaryRoot -Recurse -Force -ErrorAction SilentlyContinue
    if ($stagingDir -and (Test-Path -LiteralPath $stagingDir)) {
      Remove-Item -LiteralPath $stagingDir -Recurse -Force -ErrorAction SilentlyContinue
    }
  }

  $installedPackage = Resolve-CodexPackageContents $versionDir $expectedTarget
  $binaryPath = $installedPackage.binaryPath

  $state = [ordered]@{
    schemaVersion = 2
    binaryPath = $binaryPath
    packageRoot = $installedPackage.packageRoot
    packageLayout = "codex_package_v1"
    target = $expectedTarget
    artifact = $assetName
    artifactSha256 = $expected
    source = [string]$asset.mirror_url
    installedAt = (Get-Date).ToUniversalTime().ToString("o")
  }
  New-Item -ItemType Directory -Force -Path $cliRoot | Out-Null
  Write-Utf8NoBomFile $statePath (($state | ConvertTo-Json -Depth 4) + "`n")
  Remove-LegacyManagedCodexCli $current $cliRoot $installedPackage.packageRoot

  $script:result.cliInstallMethod = "baijimu-cache-official-cli"
  $script:result.cliArtifact = $assetName
  $script:result.cliArtifactSha256 = $expected
  return $binaryPath
}

function Resolve-CodexCli {
  $configured = Get-ConfiguredCodexCli
  if ($configured) { return $configured }

  $system = Get-SystemCodexCli
  if ($system) { return $system }

  return Install-CodexCliFromBaijimuCache
}

function Set-CodexProcessCommand([System.Diagnostics.ProcessStartInfo]$psi, [string]$codexExe, [string]$arguments) {
  $extension = [System.IO.Path]::GetExtension($codexExe)
  if ($extension -in @(".cmd", ".bat")) {
    $psi.FileName = if ($env:ComSpec) { $env:ComSpec } else { "cmd.exe" }
    $psi.Arguments = "/d /s /c `"`"$codexExe`" $arguments`""
    return
  }
  $psi.FileName = $codexExe
  $psi.Arguments = $arguments
}

function Stop-CodexProcess([System.Diagnostics.Process]$process) {
  if ($process.HasExited) { return }
  try {
    & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
  } catch {}
  if (-not $process.HasExited) {
    try { $process.Kill() } catch {}
  }
  $global:LASTEXITCODE = 0
}

function Invoke-CodexProcess([string]$codexExe, [string]$arguments, [int]$timeoutSeconds) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  Set-CodexProcessCommand $psi $codexExe $arguments
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false
  $psi.CreateNoWindow = $true
  $proc = [System.Diagnostics.Process]::Start($psi)
  if (-not $proc.WaitForExit($timeoutSeconds * 1000)) {
    Stop-CodexProcess $proc
    return @{
      timedOut = $true
      exitCode = $null
      stdout = ""
      stderr = "timed out after $timeoutSeconds seconds"
    }
  }
  return @{
    timedOut = $false
    exitCode = $proc.ExitCode
    stdout = $proc.StandardOutput.ReadToEnd()
    stderr = $proc.StandardError.ReadToEnd()
  }
}

function Invoke-AppServerLogin([string]$codexExe) {
  Set-InstallStep 8 "running" "Verifying isolated Codex profile with the official CLI"
  $apiKey = Get-CodexRouterApiKey
  $script:result.codexExe = $codexExe
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  Set-CodexProcessCommand $psi $codexExe "app-server"
  $psi.RedirectStandardInput = $true
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false
  $psi.CreateNoWindow = $true
  $appServerUtf8NoBom = New-Object System.Text.UTF8Encoding($false)
  $previousConsoleInputEncoding = [Console]::InputEncoding
  try {
    [Console]::InputEncoding = $appServerUtf8NoBom
    $proc = [System.Diagnostics.Process]::Start($psi)
    $appServerInput = $proc.StandardInput
  } finally {
    [Console]::InputEncoding = $previousConsoleInputEncoding
  }
  $appServerInput.AutoFlush = $true
  $stderrTask = $proc.StandardError.ReadToEndAsync()
  $deadline = [DateTime]::UtcNow.AddSeconds(30)
  $protocolStage = "initialize response"

  function Send-JsonRpcLine([string]$json) {
    if ($proc.HasExited) {
      throw "Codex app-server exited before sending $protocolStage (exit code $($proc.ExitCode))"
    }
    $appServerInput.WriteLine($json)
  }

  function Mask-AppServerText([AllowEmptyString()][string]$text) {
    if ([string]::IsNullOrEmpty($text)) { return "" }
    return $text.Replace($apiKey, "***")
  }

  function Read-AppServerMessage {
    while ($true) {
      $remainingMs = [int][Math]::Ceiling(($deadline - [DateTime]::UtcNow).TotalMilliseconds)
      if ($remainingMs -le 0) {
        throw "Timed out waiting for Codex app-server $protocolStage"
      }

      $readTask = $proc.StandardOutput.ReadLineAsync()
      if (-not $readTask.Wait($remainingMs)) {
        throw "Timed out waiting for Codex app-server $protocolStage"
      }
      $line = $readTask.Result
      if ($null -eq $line) {
        $exitDetail = if ($proc.HasExited) { "exit code $($proc.ExitCode)" } else { "stdout closed" }
        throw "Codex app-server ended before $protocolStage ($exitDetail)"
      }

      try {
        return ($line | ConvertFrom-Json -ErrorAction Stop)
      } catch {
        $maskedLine = Mask-AppServerText $line
        if ($maskedLine.Length -gt 240) { $maskedLine = $maskedLine.Substring(0, 240) }
        Add-Warning "ignored non-JSON Codex app-server stdout while waiting for ${protocolStage}: $maskedLine"
      }
    }
  }

  function Get-AppServerError([object]$message) {
    if (-not $message.PSObject.Properties["error"] -or $null -eq $message.error) { return $null }
    $code = if ($message.error.PSObject.Properties["code"]) { [string]$message.error.code } else { "unknown" }
    $detail = if ($message.error.PSObject.Properties["message"]) { [string]$message.error.message } else { ($message.error | ConvertTo-Json -Compress -Depth 6) }
    return "JSON-RPC error ${code}: $(Mask-AppServerText $detail)"
  }

  $protocolError = $null
  try {
    Send-JsonRpcLine '{"method":"initialize","id":0,"params":{"clientInfo":{"name":"baijimu-installer","title":"Baijimu Installer","version":"1.0.0"}}}'
    while ($true) {
      $message = Read-AppServerMessage
      if ([string]$message.id -ne "0") { continue }
      $rpcError = Get-AppServerError $message
      if ($rpcError) { throw "Codex app-server initialize failed: $rpcError" }
      break
    }

    Send-JsonRpcLine '{"method":"initialized","params":{}}'
    $protocolStage = "API-key login completion"
    $loginRequest = [ordered]@{
      method = "account/login/start"
      id = 2
      params = [ordered]@{ type = "apiKey"; apiKey = $apiKey }
    } | ConvertTo-Json -Compress -Depth 4
    Send-JsonRpcLine $loginRequest
    Remove-Variable loginRequest -ErrorAction SilentlyContinue

    $loginResponse = $false
    $loginCompleted = $false
    $authModeUpdated = $false
    while (-not ($loginResponse -and $loginCompleted -and $authModeUpdated)) {
      $message = Read-AppServerMessage
      if ([string]$message.id -eq "2") {
        $rpcError = Get-AppServerError $message
        if ($rpcError) { throw "Codex app-server API-key login request failed: $rpcError" }
        $loginType = if ($message.result -and $message.result.PSObject.Properties["type"]) { [string]$message.result.type } else { "" }
        if ($loginType -ne "apiKey") {
          throw "Codex app-server API-key login returned unexpected type '$loginType'"
        }
        $loginResponse = $true
        $script:result.appServerLoginResponse = $true
        continue
      }
      if ([string]$message.method -eq "account/login/completed") {
        $success = $message.params -and $message.params.success -eq $true
        if (-not $success) {
          $detail = if ($message.params -and $message.params.PSObject.Properties["error"] -and $message.params.error) { [string]$message.params.error } else { "unknown error" }
          throw "Codex app-server API-key login was rejected: $(Mask-AppServerText $detail)"
        }
        $loginCompleted = $true
        $script:result.appServerLogin = $true
        continue
      }
      if ([string]$message.method -eq "account/updated") {
        $authMode = if ($message.params -and $message.params.PSObject.Properties["authMode"]) { [string]$message.params.authMode } else { "" }
        if ($authMode -eq "apikey") {
          $authModeUpdated = $true
          $script:result.appServerAuthModeUpdated = $true
        }
      }
    }

    $protocolStage = "account/read API-key state"
    Send-JsonRpcLine '{"method":"account/read","id":3,"params":{"refreshToken":false}}'
    while ($true) {
      $message = Read-AppServerMessage
      if ([string]$message.id -ne "3") { continue }
      $rpcError = Get-AppServerError $message
      if ($rpcError) { throw "Codex app-server account read failed: $rpcError" }
      $accountType = if ($message.result -and $message.result.account -and $message.result.account.PSObject.Properties["type"]) { [string]$message.result.account.type } else { "" }
      $script:result.appServerAccountType = if ($accountType) { $accountType } else { $null }
      if ($accountType -ne "apiKey") {
        throw "Codex app-server account read returned unexpected type '$accountType'"
      }
      break
    }
  } catch {
    $protocolError = Mask-AppServerText $_.Exception.Message
  } finally {
    try { $appServerInput.Close() } catch {}
    if (-not $proc.HasExited) { Stop-CodexProcess $proc }
    try { [void]$stderrTask.Wait(2000) } catch {
      if (-not $protocolError) {
        $protocolError = "Failed to read Codex app-server diagnostics: $($_.Exception.Message)"
      }
    }
  }

  if ($protocolError) {
    $stderr = if ($stderrTask.IsCompleted) { Mask-AppServerText $stderrTask.Result } else { "" }
    $stderr = ($stderr -split "`r?`n" | Where-Object { $_.Trim() } | Select-Object -Last 3) -join " | "
    if ($stderr.Length -gt 400) { $stderr = $stderr.Substring(0, 400) }
    if ($stderr) { throw "$protocolError; stderr: $stderr" }
    throw $protocolError
  }
  Set-InstallStep 8 "completed" "Isolated Codex profile verified with Baijimu account"
}

function Test-CodexCli([string]$codexExe) {
  Set-InstallStep 9 "running" "Checking Codex CLI version"
  $versionResult = Invoke-CodexProcess $codexExe "--version" 20
  if ($versionResult.timedOut -or $versionResult.exitCode -ne 0) {
    throw "codex --version failed: $($versionResult.stderr)"
  }
  $script:result.cliVersion = ($versionResult.stdout + $versionResult.stderr).Trim()

  $smokeResult = Invoke-CodexProcess $codexExe 'exec --skip-git-repo-check "Reply exactly OK"' 90
  $smokeText = ($smokeResult.stdout + "`n" + $smokeResult.stderr).Trim()
  if ($smokeResult.timedOut -or $smokeResult.exitCode -ne 0 -or $smokeText -notmatch '\bOK\b') {
    throw "codex exec smoke test failed: $($smokeResult.stderr)"
  }
  $script:result.cliSmoke = $true
  Set-InstallStep 9 "completed" "Codex CLI verified"
}

function Test-VisibleWindow {
  if ($env:CODEX_INSTALL_SKIP_DESKTOP_RESTART -eq "1") {
    Set-InstallStep 10 "skipped" "Existing ChatGPT desktop session was preserved"
    return
  }
  Set-InstallStep 10 "running" "Starting and verifying ChatGPT Codex window"
  Get-CodexProcesses | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 1
  Start-CodexDesktop
  $processes = @()
  $visibleWindows = @()
  $deadline = (Get-Date).AddSeconds(45)
  do {
    Start-Sleep -Seconds 3
    $processes = @(Get-CodexProcesses)
    $visibleWindows = @(Get-CodexVisibleWindows)
  } while ($visibleWindows.Count -eq 0 -and (Get-Date) -lt $deadline)
  $script:result.processCount = $processes.Count
  $script:result.visibleWindow = $visibleWindows.Count -gt 0
  if (-not $script:result.visibleWindow) {
    throw "ChatGPT desktop app started but no visible window handle was detected"
  }
  Set-InstallStep 10 "completed" "ChatGPT Codex window is visible"
}

try {
  Ensure-CodexApp
  Write-CodexConfig
  Test-Router
  $codexExe = Resolve-CodexCli
  Invoke-AppServerLogin $codexExe
  Test-CodexCli $codexExe
  Test-VisibleWindow
} catch {
  Add-Error $_.Exception.Message
  if ($script:CurrentStepIndex -gt 0) {
    Set-InstallStep $script:CurrentStepIndex "failed" $_.Exception.Message
  }
}

$stopwatch.Stop()
$result.elapsedMs = [int]$stopwatch.ElapsedMilliseconds
$result.ok = ($result.errors.Count -eq 0)
$resultJson = $result | ConvertTo-Json -Depth 6
Write-Utf8NoBomFile $resultPath ($resultJson + "`n")
if ($result.ok) {
  Complete-PendingInstallSteps "skipped" "Install completed"
  Write-InstallConsole ""
  Write-InstallConsole "ChatGPT desktop app and Codex setup completed. You can close this window."
} else {
  Complete-PendingInstallSteps "skipped" "Install stopped"
  Write-InstallConsole ""
  Write-InstallConsole "ChatGPT desktop app and Codex setup failed. Please send the error to Baijimu."
}
$resultJson

if (-not $result.ok) {
  exit 1
}
