param(
  [Parameter(Mandatory = $true)]
  [string]$ScriptPath
)

$ErrorActionPreference = "Stop"

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

foreach ($name in @("Resolve-CodexPackageContents", "Remove-LegacyManagedCodexCli")) {
  $definition = $ast.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
      $node.Name -eq $name
  }, $true) | Select-Object -First 1
  if (-not $definition) { throw "Installer script is missing function $name" }
  Invoke-Expression $definition.Extent.Text
}

$script:Warnings = @()
function Add-Warning([string]$message) { $script:Warnings += $message }

$testRoot = Join-Path $env:RUNNER_TEMP "codex-installer-package-layout-$([Guid]::NewGuid().ToString('N'))"
$packageRoot = Join-Path $testRoot "package"
$target = "x86_64-pc-windows-msvc"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

try {
  foreach ($directory in @("bin", "codex-resources", "codex-path")) {
    New-Item -ItemType Directory -Force -Path (Join-Path $packageRoot $directory) | Out-Null
  }
  $metadata = [ordered]@{
    layoutVersion = 1
    version = "test"
    target = $target
    variant = "codex"
    entrypoint = "bin/codex.exe"
    resourcesDir = "codex-resources"
    pathDir = "codex-path"
  } | ConvertTo-Json -Depth 4
  [System.IO.File]::WriteAllText((Join-Path $packageRoot "codex-package.json"), $metadata, $utf8NoBom)
  foreach ($relativePath in @(
    "bin\codex.exe",
    "bin\codex-code-mode-host.exe",
    "codex-resources\codex-command-runner.exe",
    "codex-resources\codex-windows-sandbox-setup.exe",
    "codex-path\rg.exe"
  )) {
    [System.IO.File]::WriteAllText((Join-Path $packageRoot $relativePath), $relativePath, $utf8NoBom)
  }

  $resolved = Resolve-CodexPackageContents $packageRoot $target
  if ($resolved.binaryPath -ne [System.IO.Path]::GetFullPath((Join-Path $packageRoot "bin\codex.exe"))) {
    throw "Package resolver did not use codex-package.json entrypoint"
  }
  if ($resolved.binaryPath -match "command-runner|sandbox-setup") {
    throw "Package resolver selected a Windows helper as the Codex entrypoint"
  }

  $runner = Join-Path $packageRoot "codex-resources\codex-command-runner.exe"
  Remove-Item -LiteralPath $runner -Force
  $incompleteError = $null
  try { [void](Resolve-CodexPackageContents $packageRoot $target) } catch { $incompleteError = $_.Exception.Message }
  if ($incompleteError -notmatch "incomplete") {
    throw "Package resolver accepted a package without the command runner"
  }

  $cliRoot = Join-Path $testRoot "managed-cli"
  $legacyVersionDir = Join-Path $cliRoot "versions\$([string]'a' * 64)"
  New-Item -ItemType Directory -Force -Path $legacyVersionDir | Out-Null
  $legacyBinary = Join-Path $legacyVersionDir "codex.exe"
  [System.IO.File]::WriteAllText($legacyBinary, "legacy", $utf8NoBom)
  $legacyState = [pscustomobject]@{
    artifact = "codex-x86_64-pc-windows-msvc.exe.zip"
    binaryPath = $legacyBinary
  }
  Remove-LegacyManagedCodexCli $legacyState $cliRoot $packageRoot
  if (Test-Path -LiteralPath $legacyVersionDir) {
    throw "Legacy flat Windows Codex cache was not removed"
  }

  Write-Host "Windows official Codex package layout verified"
} finally {
  Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
