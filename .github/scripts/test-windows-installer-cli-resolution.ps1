param(
  [Parameter(Mandatory = $true)]
  [string]$ScriptPath
)

$ErrorActionPreference = "Stop"

$tokens = $null
$parseErrors = $null
$installerSource = [System.IO.File]::ReadAllText($ScriptPath, [System.Text.Encoding]::UTF8)
$ast = [System.Management.Automation.Language.Parser]::ParseInput(
  $installerSource,
  [ref]$tokens,
  [ref]$parseErrors
)
if ($parseErrors.Count -gt 0) {
  throw "Installer script has PowerShell parse errors: $($parseErrors[0].Message)"
}

foreach ($name in @(
  "Set-CodexProcessCommand",
  "Stop-CodexProcess",
  "Invoke-CodexProcess",
  "Test-CodexCliCandidate",
  "Get-SystemCodexCli"
)) {
  $definition = $ast.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
      $node.Name -eq $name
  }, $true) | Select-Object -First 1
  if (-not $definition) { throw "Installer script is missing function $name" }
  Invoke-Expression $definition.Extent.Text
}

$script:Warnings = @()
$script:result = @{}
function Add-Warning([string]$message) { $script:Warnings += $message }

$testRoot = Join-Path $env:RUNNER_TEMP "codex-installer-cli-resolution-$([Guid]::NewGuid().ToString('N'))"
$brokenBin = Join-Path $testRoot "broken"
$workingBin = Join-Path $testRoot "working"
$originalPath = $env:PATH
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

try {
  New-Item -ItemType Directory -Force -Path $brokenBin, $workingBin | Out-Null
  [System.IO.File]::WriteAllText(
    (Join-Path $brokenBin "codex.cmd"),
    "@echo off`r`nexit /b 1`r`n",
    $utf8NoBom
  )
  [System.IO.File]::WriteAllText(
    (Join-Path $workingBin "codex.cmd"),
    "@echo off`r`nif `%1==--version (`r`n  echo codex-cli 1.2.3`r`n  exit /b 0`r`n)`r`nif `%1==app-server if `%2==--help exit /b 0`r`nexit /b 1`r`n",
    $utf8NoBom
  )

  $env:PATH = "$brokenBin;$workingBin;$env:SystemRoot\System32"
  $resolved = Get-SystemCodexCli
  $expected = [System.IO.Path]::GetFullPath((Join-Path $workingBin "codex.cmd"))
  if ($resolved -ne $expected) {
    throw "System resolver did not skip the broken Codex command: resolved=$resolved expected=$expected"
  }
  if ($script:result.cliInstallMethod -ne "already-installed") {
    throw "System resolver did not record the validated installation method"
  }
  if ($script:Warnings.Count -ne 1 -or $script:Warnings[0] -notmatch "codex --version") {
    throw "System resolver did not report why the broken Codex command was skipped"
  }

  $script:Warnings = @()
  $script:result = @{}
  $env:PATH = "$brokenBin;$env:SystemRoot\System32"
  if ($null -ne (Get-SystemCodexCli)) {
    throw "System resolver accepted a Codex command whose version probe failed"
  }
  if ($script:result.ContainsKey("cliInstallMethod")) {
    throw "System resolver recorded an install method for an unusable Codex command"
  }

  Write-Host "Windows Codex CLI resolution verified"
} finally {
  $env:PATH = $originalPath
  Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
