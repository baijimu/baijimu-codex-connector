param(
  [Parameter(Mandatory = $true)]
  [string]$BinaryPath
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
  throw "Codex Connector binary is missing: $BinaryPath"
}

function Get-FreePort {
  $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
  $listener.Start()
  try {
    return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
  } finally {
    $listener.Stop()
  }
}

function Invoke-ConnectorRequest {
  param(
    [Parameter(Mandatory = $true)]
    [int]$Port,
    [Parameter(Mandatory = $true)]
    [string]$Path
  )
  $client = [System.Net.Http.HttpClient]::new()
  $client.Timeout = [TimeSpan]::FromSeconds(2)
  try {
    $response = $client.GetAsync("http://127.0.0.1:$Port$Path").GetAwaiter().GetResult()
    $content = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    return [pscustomobject]@{
      Status = [int]$response.StatusCode
      Body = $content | ConvertFrom-Json
    }
  } finally {
    $client.Dispose()
  }
}

function Wait-ConnectorResponse {
  param(
    [Parameter(Mandatory = $true)]
    [int]$Port,
    [Parameter(Mandatory = $true)]
    [string]$Path,
    [Parameter(Mandatory = $true)]
    [scriptblock]$Accept,
    [int]$TimeoutSeconds = 15
  )
  $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
  $lastError = "no response"
  while ([DateTime]::UtcNow -lt $deadline) {
    try {
      $result = Invoke-ConnectorRequest -Port $Port -Path $Path
      if (& $Accept $result) {
        return $result
      }
      $lastError = "HTTP $($result.Status): $($result.Body | ConvertTo-Json -Compress -Depth 10)"
    } catch {
      $lastError = $_.Exception.Message
    }
    Start-Sleep -Milliseconds 100
  }
  throw "Connector $Path did not reach the expected state: $lastError"
}

function Start-TestConnector {
  param(
    [int]$DelayMs = 0,
    [string]$Failure = ""
  )
  $port = Get-FreePort
  $directory = Join-Path $env:RUNNER_TEMP "codex-connector-lifecycle-$([guid]::NewGuid().ToString('N'))"
  New-Item -ItemType Directory -Force -Path $directory | Out-Null
  $stdout = Join-Path $directory "stdout.log"
  $stderr = Join-Path $directory "stderr.log"
  $saved = @{
    BAIJIMU_CONNECTOR_DATA_DIR = $env:BAIJIMU_CONNECTOR_DATA_DIR
    CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN = $env:CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN
    CODEX_CONNECTOR_TEST_SKIP_RECONCILE = $env:CODEX_CONNECTOR_TEST_SKIP_RECONCILE
    CODEX_CONNECTOR_TEST_STARTUP_DELAY_MS = $env:CODEX_CONNECTOR_TEST_STARTUP_DELAY_MS
    CODEX_CONNECTOR_TEST_STARTUP_FAILURE = $env:CODEX_CONNECTOR_TEST_STARTUP_FAILURE
  }
  try {
    $env:BAIJIMU_CONNECTOR_DATA_DIR = $directory
    $env:CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN = "1"
    $env:CODEX_CONNECTOR_TEST_SKIP_RECONCILE = "1"
    $env:CODEX_CONNECTOR_TEST_STARTUP_DELAY_MS = "$DelayMs"
    $env:CODEX_CONNECTOR_TEST_STARTUP_FAILURE = $Failure
    $process = Start-Process `
      -FilePath $BinaryPath `
      -ArgumentList @("start", "--port", "$port") `
      -PassThru `
      -NoNewWindow `
      -RedirectStandardOutput $stdout `
      -RedirectStandardError $stderr
  } finally {
    foreach ($name in $saved.Keys) {
      if ($null -eq $saved[$name]) {
        Remove-Item "Env:$name" -ErrorAction SilentlyContinue
      } else {
        Set-Item "Env:$name" $saved[$name]
      }
    }
  }
  return [pscustomobject]@{
    Port = $port
    Directory = $directory
    Process = $process
    Stdout = $stdout
    Stderr = $stderr
  }
}

function Stop-TestConnector {
  param([Parameter(Mandatory = $true)]$Context)
  try {
    if (-not $Context.Process.HasExited) {
      & $BinaryPath stop --port "$($Context.Port)" | Out-Null
      if (-not $Context.Process.WaitForExit(10000)) {
        throw "Connector process did not stop within 10 seconds"
      }
    }
  } finally {
    if (-not $Context.Process.HasExited) {
      $Context.Process.Kill($true)
      $Context.Process.WaitForExit()
    }
    if ($Context.Process.ExitCode -ne 0) {
      $stderr = if (Test-Path -LiteralPath $Context.Stderr) {
        Get-Content -LiteralPath $Context.Stderr -Raw
      } else { "" }
      throw "Connector exited with $($Context.Process.ExitCode): $stderr"
    }
    Remove-Item -LiteralPath $Context.Directory -Recurse -Force -ErrorAction SilentlyContinue
  }
}

$delayed = Start-TestConnector -DelayMs 1500
try {
  $live = Wait-ConnectorResponse -Port $delayed.Port -Path "/healthz" -Accept {
    param($response)
    $response.Status -eq 200 -and $response.Body.status.startup.status -eq "initializing"
  } -TimeoutSeconds 5
  if (-not $live.Body.ok) {
    throw "Liveness endpoint must remain healthy while initialization is running"
  }
  $initializing = Invoke-ConnectorRequest -Port $delayed.Port -Path "/readyz"
  if ($initializing.Status -ne 503 -or $initializing.Body.error.code -ne "connector_initializing") {
    throw "Readiness endpoint did not report initialization in progress"
  }
  Wait-ConnectorResponse -Port $delayed.Port -Path "/readyz" -Accept {
    param($response)
    $response.Status -eq 200 -and $response.Body.status.startup.status -eq "ready"
  } | Out-Null
} finally {
  Stop-TestConnector -Context $delayed
}

$expectedFailure = "injected Windows CODEX_HOME synchronization failure"
$failed = Start-TestConnector -Failure $expectedFailure
try {
  Wait-ConnectorResponse -Port $failed.Port -Path "/healthz" -Accept {
    param($response)
    $response.Status -eq 200
  } | Out-Null
  $failure = Wait-ConnectorResponse -Port $failed.Port -Path "/readyz" -Accept {
    param($response)
    $response.Status -eq 503 -and $response.Body.status.startup.status -eq "failed"
  }
  if ($failure.Body.error.code -ne "connector_initialization_failed") {
    throw "Readiness endpoint did not classify the initialization failure"
  }
  if ($failure.Body.error.message -ne $expectedFailure) {
    throw "Readiness endpoint lost the initialization root cause"
  }
} finally {
  Stop-TestConnector -Context $failed
}

Write-Host "Windows Codex Connector liveness, readiness, failure diagnostics, and stop verified"
