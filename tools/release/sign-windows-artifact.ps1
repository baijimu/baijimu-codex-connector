param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath
)

$ErrorActionPreference = "Stop"

function Protect-DiagnosticText {
    param([string]$Text)

    $protectedText = $Text
    foreach ($name in @("SSL_COM_USERNAME", "SSL_COM_PASSWORD", "SSL_COM_CREDENTIAL_ID", "SSL_COM_TOTP_SECRET")) {
        $secret = [Environment]::GetEnvironmentVariable($name)
        if (-not [string]::IsNullOrEmpty($secret)) {
            $protectedText = $protectedText.Replace($secret, "***")
        }
    }
    return $protectedText
}

foreach ($name in @("SSL_COM_USERNAME", "SSL_COM_PASSWORD", "SSL_COM_CREDENTIAL_ID", "SSL_COM_TOTP_SECRET", "CODESIGN_TOOL_PATH")) {
    if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))) {
        throw "Missing required Windows signing environment variable: $name"
    }
}

$resolvedFile = Resolve-Path -LiteralPath $FilePath
$codeSignTool = Resolve-Path -LiteralPath $env:CODESIGN_TOOL_PATH
$codeSignToolDirectory = Split-Path -Parent $codeSignTool.Path
$javaExecutable = Get-ChildItem `
    -Path $codeSignToolDirectory `
    -Filter "java.exe" `
    -File `
    -Recurse `
    -ErrorAction Stop |
    Where-Object { $_.FullName -match "\\jdk-[^\\]+\\bin\\java\.exe$" } |
    Select-Object -First 1
$codeSignToolJar = Get-ChildItem `
    -Path (Join-Path $codeSignToolDirectory "jar") `
    -Filter "code_sign_tool-*.jar" `
    -File `
    -ErrorAction Stop |
    Select-Object -First 1
if (-not $javaExecutable) {
    throw "Unable to locate the Java runtime bundled with CodeSignTool"
}
if (-not $codeSignToolJar) {
    throw "Unable to locate the CodeSignTool JAR"
}

$workingDirectory = Join-Path ([System.IO.Path]::GetTempPath()) "codex-codesign-$([guid]::NewGuid().ToString('N'))"
$inputDirectory = Join-Path $workingDirectory "input"
$outputDirectory = Join-Path $workingDirectory "output"
New-Item -ItemType Directory -Force -Path $inputDirectory, $outputDirectory | Out-Null
$stagedInputFile = Join-Path $inputDirectory "codex-signing-input.exe"
$replacementFile = "$($resolvedFile.Path).signed.tmp"
$unsignedBackupFile = "$($resolvedFile.Path).unsigned.bak"
Copy-Item -Force -LiteralPath $resolvedFile.Path -Destination $stagedInputFile

$arguments = @(
    "-Dfile.encoding=UTF-8",
    "-jar",
    $codeSignToolJar.FullName,
    "sign",
    "-username=$env:SSL_COM_USERNAME",
    "-password=$env:SSL_COM_PASSWORD",
    "-credential_id=$env:SSL_COM_CREDENTIAL_ID",
    "-totp_secret=$env:SSL_COM_TOTP_SECRET",
    "-input_file_path=$stagedInputFile",
    "-output_dir_path=$outputDirectory"
)

try {
    Push-Location $codeSignToolDirectory
    try {
        $toolOutput = & $javaExecutable.FullName @arguments 2>&1
        $toolExitCode = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    foreach ($line in $toolOutput) {
        Write-Host (Protect-DiagnosticText -Text $line.ToString())
    }
    if ($toolExitCode -ne 0) {
        throw "CodeSignTool failed with exit code $toolExitCode"
    }
    $signedFile = Join-Path $outputDirectory (Split-Path -Leaf $stagedInputFile)
    if (-not (Test-Path -LiteralPath $signedFile)) {
        throw "CodeSignTool did not produce the expected signed artifact"
    }
    Copy-Item -Force -LiteralPath $signedFile -Destination $replacementFile
    [System.IO.File]::Replace($replacementFile, $resolvedFile.Path, $unsignedBackupFile)
} finally {
    if (Test-Path -LiteralPath $workingDirectory) {
        Remove-Item -Force -Recurse -LiteralPath $workingDirectory
    }
    if (Test-Path -LiteralPath $replacementFile) {
        Remove-Item -Force -LiteralPath $replacementFile
    }
    if (Test-Path -LiteralPath $unsignedBackupFile) {
        Remove-Item -Force -LiteralPath $unsignedBackupFile
    }
}
