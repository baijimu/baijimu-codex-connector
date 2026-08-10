$ErrorActionPreference = "Stop"

$setupSource = Get-Content -LiteralPath "src/setup.rs" -Raw
$urlMatch = [regex]::Match(
  $setupSource,
  'const WINDOWS_SCRIPT_URL: &str =\s*\r?\n\s*"([^"]+)";'
)
$shaMatch = [regex]::Match(
  $setupSource,
  'const WINDOWS_SCRIPT_SHA256: &str =\s*\r?\n\s*"([0-9a-f]{64})";'
)
if (-not $urlMatch.Success -or -not $shaMatch.Success) {
  throw "Unable to read the pinned Windows installer identity from src/setup.rs"
}

$scriptPath = Join-Path $env:RUNNER_TEMP "windows-configure-terminal-and-login.ps1"
& curl.exe `
  --fail `
  --location `
  --silent `
  --show-error `
  --retry 6 `
  --retry-all-errors `
  --retry-delay 3 `
  --connect-timeout 15 `
  --max-time 120 `
  --output $scriptPath `
  $urlMatch.Groups[1].Value
if ($LASTEXITCODE -ne 0) {
  throw "Unable to download the pinned Windows installer: curl.exe exit $LASTEXITCODE"
}
$actualSha256 = (Get-FileHash -LiteralPath $scriptPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualSha256 -ne $shaMatch.Groups[1].Value) {
  throw "Pinned Windows installer SHA-256 mismatch: $actualSha256"
}

$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
  $scriptPath,
  [ref]$tokens,
  [ref]$parseErrors
)
if ($parseErrors.Count -gt 0) {
  throw "Pinned Windows installer does not parse: $($parseErrors[0].Message)"
}

$writer = $ast.Find(
  {
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
      $node.Name -eq "Write-Utf8NoBomFile"
  },
  $true
)
$encodingAssignment = $ast.EndBlock.Statements | Where-Object {
  $_.Extent.Text -match '^\$script:Utf8NoBomEncoding\s*='
} | Select-Object -First 1
if (-not $writer -or -not $encodingAssignment) {
  throw "Pinned Windows installer is missing the atomic UTF-8 writer"
}
if ($writer.Extent.Text -notmatch '\[System\.Management\.Automation\.Language\.NullString\]::Value') {
  throw "Atomic writer does not pass a real null backup path to File.Replace"
}
if ($writer.Extent.Text -match 'File\]::Replace\([\s\S]*?\$null\s*,\s*\$true') {
  throw "Atomic writer still passes PowerShell null to a .NET string parameter"
}

. ([scriptblock]::Create("$($encodingAssignment.Extent.Text)`n$($writer.Extent.Text)"))
$testDirectory = Join-Path $env:RUNNER_TEMP "codex-installer-atomic-write"
New-Item -ItemType Directory -Force -Path $testDirectory | Out-Null
$target = Join-Path $testDirectory "state.json"
try {
  Write-Utf8NoBomFile $target '{"attempt":1}'
  Write-Utf8NoBomFile $target '{"attempt":2}'

  $bytes = [System.IO.File]::ReadAllBytes($target)
  if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xef -and $bytes[1] -eq 0xbb -and $bytes[2] -eq 0xbf) {
    throw "Atomic writer emitted a UTF-8 BOM"
  }
  if ([System.IO.File]::ReadAllText($target) -ne '{"attempt":2}') {
    throw "Atomic writer did not replace the existing file"
  }
  $temporaryFiles = @(Get-ChildItem -LiteralPath $testDirectory -Filter ".state.json.tmp-*" -File)
  if ($temporaryFiles.Count -ne 0) {
    throw "Atomic writer left temporary files behind"
  }
} finally {
  Remove-Item -LiteralPath $testDirectory -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "Windows PowerShell 5.1 atomic installer writer verified"
