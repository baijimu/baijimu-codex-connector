import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readdir, readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

test("connector owns its application release and upstream artifact sync workflows", async () => {
  const workflowDirectory = join(root, ".github", "workflows");
  const workflowFiles = (await readdir(workflowDirectory)).filter((name) =>
    /\.ya?ml$/.test(name),
  );
  assert.deepEqual(workflowFiles, [
    "release.yml",
    "sync-codex-upstream-artifacts.yml",
  ]);

  const workflow = await readFile(join(workflowDirectory, "release.yml"), "utf8");
  assert.match(workflow, /tags:\s*\n\s*- "v\*"/);
  assert.match(workflow, /github\.event_name == 'push' \|\| inputs\.publish/);
  assert.match(workflow, /jobs:\s*\n\s*validate:/);
  for (const job of [
    "build",
    "prepare-release",
    "publish-oss",
    "publish-release",
    "publish-market",
    "verify-published",
  ]) {
    assert.match(workflow, new RegExp(`\\n  ${job}:`));
  }
  for (const secret of [
    "APPLE_CERTIFICATE",
    "APPLE_CERTIFICATE_PASSWORD",
    "SSL_COM_USERNAME",
    "SSL_COM_PASSWORD",
    "SSL_COM_CREDENTIAL_ID",
    "SSL_COM_TOTP_SECRET",
    "OSS_ACCESS_KEY_ID",
    "OSS_ACCESS_KEY_SECRET",
    "LOCAL_APP_MARKET_PUBLISH_TOKEN",
  ]) {
    assert.match(workflow, new RegExp(`secrets\\.${secret}`));
  }
  assert.doesNotMatch(workflow, /codex-local-app-v/);
  assert.doesNotMatch(workflow, /release-codex-local-app/);
  assert.doesNotMatch(workflow, /Jenkins/i);
  assert.doesNotMatch(workflow, /gitee\.com|zxflimit_admin/);
  assert.match(workflow, /BAIJIMU_CLI_VERSION: "0\.1\.43"/);
  assert.match(workflow, /c3b73aeeef5a03166eef784d06a0825386b245d012add0910db8ee68d2447add/);
  assert.match(workflow, /managed-tool-artifacts\/baijimu-cli\/releases\/v0\.1\.43/);
  assert.doesNotMatch(workflow, /bridge-agent\/releases/);
  assert.match(workflow, /git merge-base --is-ancestor "\$sha" origin\/main/);
  assert.match(workflow, /needs\.validate\.outputs\.verify == 'true'/);
  assert.match(workflow, /published-release\.json/);
  assert.match(workflow, /\.assets\[\] \| \[\.name, \.url\]/);
  assert.match(workflow, /RUSTFLAGS: "-C target-feature=\+crt-static -D warnings"/);
  assert.match(workflow, /\$PSNativeCommandUseErrorActionPreference = \$true/);
  assert.match(workflow, /Verify Windows binaries are self-contained/);
  assert.match(workflow, /Verify packaged Windows Connector lifecycle/);
  assert.match(workflow, /test-windows-connector-lifecycle\.ps1/);
  assert.match(workflow, /Verify installer atomic writes with Windows PowerShell 5\.1/);
  assert.match(workflow, /shell: powershell/);
  assert.match(
    workflow,
    /Verify installer atomic writes with Windows PowerShell 5\.1[\s\S]*?timeout-minutes: 5/,
  );
  assert.match(workflow, /test-windows-installer-atomic-write\.ps1/);
  assert.match(
    workflow,
    /Verify installer app-server login protocol with Windows PowerShell 5\.1[\s\S]*?timeout-minutes: 5/,
  );
  assert.match(workflow, /test-windows-installer-app-server-login\.ps1/);
  assert.match(
    workflow,
    /Verify official Codex package layout with Windows PowerShell 5\.1[\s\S]*?timeout-minutes: 5/,
  );
  assert.match(workflow, /test-windows-installer-package-layout\.ps1/);
  assert.doesNotMatch(workflow, /Upload validated installer scripts/);
  assert.doesNotMatch(workflow, /Download validated installer scripts/);
  assert.match(workflow, /installers\\windows-configure-terminal-and-login\.ps1/);
  const windowsInstallerTest = await readFile(
    join(root, ".github", "scripts", "test-windows-installer-atomic-write.ps1"),
    "utf8",
  );
  assert.doesNotMatch(windowsInstallerTest, /Invoke-WebRequest/);
  assert.doesNotMatch(windowsInstallerTest, /curl\.exe|https?:\/\//);
  assert.match(windowsInstallerTest, /Parameter\(Mandatory = \$true\)/);
  assert.match(windowsInstallerTest, /Test-Path -LiteralPath \$ScriptPath -PathType Leaf/);
  const windowsLoginTest = await readFile(
    join(root, ".github", "scripts", "test-windows-installer-app-server-login.ps1"),
    "utf8",
  );
  assert.doesNotMatch(windowsLoginTest, /Invoke-WebRequest|curl\.exe|https?:\/\//);
  assert.match(windowsLoginTest, /delayed-success/);
  assert.match(windowsLoginTest, /Start-Sleep -Seconds 3/);
  assert.match(windowsLoginTest, /denied by fake server/);
  assert.match(windowsLoginTest, /exposed the API key/);
  const windowsPackageTest = await readFile(
    join(root, ".github", "scripts", "test-windows-installer-package-layout.ps1"),
    "utf8",
  );
  assert.doesNotMatch(windowsPackageTest, /Invoke-WebRequest|curl\.exe|https?:\/\//);
  assert.match(windowsPackageTest, /Resolve-CodexPackageContents/);
  assert.match(windowsPackageTest, /codex-command-runner\.exe/);
  assert.match(windowsPackageTest, /Legacy flat Windows Codex cache was not removed/);
  assert.doesNotMatch(windowsInstallerTest, /Get-FileHash -LiteralPath \$ScriptPath/);
  assert.match(workflow, /Verify embedded installer scripts/);
  assert.match(workflow, /installers\/macos-configure-terminal-and-login\.sh/);
  assert.match(workflow, /installers\/windows-configure-terminal-and-login\.ps1/);
  assert.match(workflow, /curl exit \$curl_status/);
  assert.match(workflow, /New-Object System\.Text\.UTF8Encoding\(\$false\)/);
  assert.match(workflow, /Write-Utf8NoBomFile \$authPath/);
  assert.match(workflow, /Write-Utf8NoBomFile \$configPath/);
  assert.match(workflow, /Write-Utf8NoBomFile \$statePath/);
  assert.match(workflow, /ReadLineAsync\(\)/);
  assert.match(workflow, /account\/read API-key state/);
  assert.match(workflow, /ConvertFrom-Json -ErrorAction Stop/);
  assert.match(workflow, /Set-Content\[\^\[:cntrl:\]\]\*\-Encoding/);
  assert.match(workflow, /dumpbin\.exe/);
  assert.match(workflow, /VCRUNTIME\|MSVCP/);
  const windowsLifecycleTest = await readFile(
    join(root, ".github", "scripts", "test-windows-connector-lifecycle.ps1"),
    "utf8",
  );
  assert.match(windowsLifecycleTest, /\/healthz/);
  assert.match(windowsLifecycleTest, /\/readyz/);
  assert.match(windowsLifecycleTest, /connector_initializing/);
  assert.match(windowsLifecycleTest, /connector_initialization_failed/);
  assert.match(windowsLifecycleTest, /CODEX_CONNECTOR_TEST_STARTUP_DELAY_MS/);
  assert.match(windowsLifecycleTest, /CODEX_CONNECTOR_TEST_STARTUP_FAILURE/);

  for (const action of ["actions/checkout", "actions/upload-artifact", "actions/download-artifact"]) {
    const pattern = new RegExp(`${action.replace("/", "\\/")}@[0-9a-f]{40}`);
    assert.match(workflow, pattern);
  }
});

test("connector compiles platform installers instead of downloading executable scripts", async () => {
  const setupSource = await readFile(join(root, "src", "setup.rs"), "utf8");
  const macosInstaller = await readFile(
    join(root, "installers", "macos-configure-terminal-and-login.sh"),
  );
  const windowsInstaller = await readFile(
    join(root, "installers", "windows-configure-terminal-and-login.ps1"),
  );

  assert.match(
    setupSource,
    /include_bytes!\("\.\.\/installers\/macos-configure-terminal-and-login\.sh"\)/,
  );
  assert.match(
    setupSource,
    /include_bytes!\("\.\.\/installers\/windows-configure-terminal-and-login\.ps1"\)/,
  );
  assert.doesNotMatch(setupSource, /CODEX_CONNECTOR_INSTALL_SCRIPT_URL/);
  assert.doesNotMatch(setupSource, /SCRIPT_URL|SCRIPT_SHA256|download_script/);
  assert.ok(macosInstaller.length > 1_000);
  assert.ok(windowsInstaller.length > 1_000);
});

test("upstream sync is release-side, complete, latest-only, and independently scheduled", async () => {
  const workflow = await readFile(
    join(root, ".github", "workflows", "sync-codex-upstream-artifacts.yml"),
    "utf8",
  );
  const wrapper = await readFile(
    join(root, "tools", "codex-artifacts", "sync-codex-artifacts.sh"),
    "utf8",
  );
  const synchronizer = await readFile(
    join(root, "tools", "codex-artifacts", "sync_codex_artifacts.py"),
    "utf8",
  );

  assert.match(workflow, /schedule:/);
  assert.match(workflow, /workflow_dispatch:/);
  assert.doesNotMatch(workflow, /push:\s*\n\s*tags:/);
  assert.match(workflow, /concurrency:\s*\n\s*group: codex-upstream-artifact-sync/);
  assert.match(workflow, /verify-macos-apps:/);
  assert.match(workflow, /codesign --verify --deep --strict/);
  assert.match(workflow, /spctl --assess --type execute/);
  assert.match(workflow, /verify-windows-apps:/);
  assert.match(workflow, /signtool\.exe/);
  assert.match(workflow, /verify \/pa \/all \/v/);
  assert.match(workflow, /AppxManifest\.xml/);
  assert.match(workflow, /OpenAI\\\.\(ChatGPT\|Codex\)/);
  assert.match(workflow, /needs: \[verify-macos-apps, verify-windows-apps\]/);
  assert.match(workflow, /sync-codex-artifacts\.sh/);
  assert.match(wrapper, /Customer installers read the published/);
  assert.match(synchronizer, /schema_version": 3/);
  assert.match(synchronizer, /assets\/sha256/);
  assert.match(synchronizer, /latest\.json/);
  assert.match(synchronizer, /Publishing this pointer last/);
  assert.match(synchronizer, /DEFAULT_BUCKET = "baijimu-lowcode-public-20260420"/);
  assert.match(synchronizer, /DEFAULT_PUBLIC_BASE = "https:\/\/download\.baijimu\.com"/);
  assert.match(
    synchronizer,
    /def public_asset_is_exact[\s\S]*?"--retry-all-errors"[\s\S]*?"--connect-timeout"[\s\S]*?"--max-time"/,
  );
  assert.match(workflow, /--connect-timeout 15 --max-time 900/);
  assert.match(synchronizer, /previous_keys - current_keys/);
  assert.doesNotMatch(synchronizer, /manifests\/sha256/);
  assert.doesNotMatch(synchronizer, /PRESERVE_EXISTING_MANIFEST/);
  for (const name of [
    "codex-app-aarch64-apple-darwin.dmg",
    "codex-app-x86_64-apple-darwin.dmg",
    "codex-app-windows-x64.msix",
    "codex-app-windows-arm64.msix",
    "codex-aarch64-apple-darwin.tar.gz",
    "codex-x86_64-apple-darwin.tar.gz",
    "codex-aarch64-pc-windows-msvc.exe.zip",
    "codex-x86_64-pc-windows-msvc.exe.zip",
    "codex-package-aarch64-pc-windows-msvc.tar.gz",
    "codex-package-x86_64-pc-windows-msvc.tar.gz",
  ]) {
    assert.match(synchronizer, new RegExp(name.replaceAll(".", "\\.")));
  }
});

test("upstream manifest builder produces one complete content-addressed snapshot", () => {
  const script = String.raw`
import importlib.util
from pathlib import Path

path = Path("tools/codex-artifacts/sync_codex_artifacts.py")
spec = importlib.util.spec_from_file_location("sync_codex_artifacts", path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

sources = []
for name in module.CLI_ASSET_NAMES:
    sources.append({
        "name": name,
        "component": "codex_cli",
        "platform": "windows" if "pc-windows" in name else "macos",
        "arch": "aarch64" if "aarch64" in name else "x86_64",
        "install_layout": "codex_package_v1" if "codex-package-" in name else ("legacy_flat_windows_archive" if "pc-windows" in name else "legacy_single_binary_archive"),
        "deprecated": name.endswith(".exe.zip"),
        "source_kind": "official_openai_github_release",
        "upstream_url": "https://example.invalid/" + name,
        "effective_upstream_url": "https://example.invalid/" + name,
        "upstream_sha256": "a" * 64,
        "sha256": "a" * 64,
        "size": 10,
        "content_type": "application/gzip",
    })
for source in module.APP_ASSETS:
    sources.append({
        **source,
        "component": "codex_desktop_app",
        "effective_upstream_url": source["upstream_url"],
        "upstream_sha256": "b" * 64,
        "sha256": "b" * 64,
        "size": 20,
        "signature_verification": "native-platform",
    })
release = {"tag_name": "rust-v-test", "published_at": "2026-01-01T00:00:00Z"}
manifest = module.manifest_for(release, sources, "https://oss.example", "codex-artifacts")
module.validate_manifest(manifest)
assert manifest["schema_version"] == 3
assert len(manifest["assets"]) == 10
assert len([item for item in manifest["assets"] if item["install_layout"] == "codex_package_v1"]) == 2
assert all("/assets/sha256/" in item["mirror_url"] for item in manifest["assets"])
assert not any("preserved_from_manifest" in item for item in manifest["assets"])
`;
  const result = spawnSync("python3", ["-c", script], {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" },
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
});

test("market publisher uses explicit immutable version creation and review submission", async () => {
  const script = await readFile(
    join(root, "tools", "release", "publish-market.sh"),
    "utf8",
  );
  assert.match(script, /local-app version create codex/);
  assert.match(script, /local-app submit codex "\$version"/);
  assert.doesNotMatch(script, /local-app publish codex/);
  assert.match(script, /momoplan\/baijimu-connector-codex/);
  assert.doesNotMatch(script, /codex-local-app-v|gitee\.com|zxflimit_admin/);
});

test("desktop launch is explicit, process-scoped, and independent from user CODEX_HOME", async () => {
  const setup = await readFile(join(root, "src", "setup.rs"), "utf8");
  const desktop = await readFile(join(root, "src", "desktop.rs"), "utf8");
  const main = await readFile(join(root, "src", "main.rs"), "utf8");
  const userEnvironment = await readFile(join(root, "src", "user_environment.rs"), "utf8");
  const deferIndex = setup.indexOf('env("CODEX_INSTALL_SKIP_DESKTOP_RESTART", "1")');

  assert.ok(deferIndex >= 0, "setup must suppress installer-owned desktop launch");
  assert.doesNotMatch(setup, /desktop::launch_and_verify/);
  assert.doesNotMatch(main, /reconcile_active_user_codex_home/);
  assert.match(main, /desktop::launch_and_verify\(&selected_home\)/);
  assert.match(main, /restart_and_verify\(&previous_home\)/);
  assert.doesNotMatch(main, /user_codex_home_synchronized/);
  assert.match(setup, /"-EncodedCommand"/);
  assert.match(setup, /"-OutputFormat"/);
  assert.match(setup, /"Text"/);
  assert.match(setup, /\[Console\]::OutputEncoding/);
  assert.match(desktop, /Start-Process -FilePath \$entry\[0\]\.executable/);
  assert.doesNotMatch(desktop, /shell:AppsFolder/);
  assert.match(desktop, /Explicit CODEX_HOME is required for isolated desktop launch/);
  assert.match(desktop, /isolate_from_connector_environment/);
  assert.match(userEnvironment, /pub fn restore_codex_home/);
  assert.doesNotMatch(userEnvironment, /pub fn activate_codex_home/);
  assert.match(desktop, /if \(\$running\.Count -eq 0\) \{ throw 'ChatGPT\/Codex desktop did not start within 45 seconds' \}/);
  assert.doesNotMatch(desktop, /MainWindowHandle/);
  assert.doesNotMatch(desktop, /visibleWindowCount|visible window/);
  assert.match(desktop, /\/Applications\/ChatGPT\.app/);
  assert.match(desktop, /Command::new\("\/usr\/bin\/open"\)/);
  assert.match(desktop, /command\.arg\("--env"\)\.arg\(assignment\)/);
  assert.match(desktop, /OsString::from\("CODEX_HOME="\)/);
  assert.match(desktop, /Command::new\("\/usr\/bin\/lsappinfo"\)/);
  assert.match(desktop, /has_running_process\(&info\)/);
  assert.match(desktop, /tell application id .* to quit/);
  assert.match(desktop, /Command::new\("\/bin\/ps"\)/);
  assert.match(desktop, /没有使用所选工作区状态目录/);
  assert.doesNotMatch(desktop, /PROJECT_REOPEN_DELAY|reopen_with_project|Documents.*Codex.*default/);
  assert.doesNotMatch(desktop, /pkill/);
});

test("workspace profile homes are short, isolated, and safely migratable", async () => {
  const credential = await readFile(join(root, "src", "credential.rs"), "utf8");
  const main = await readFile(join(root, "src", "main.rs"), "utf8");
  assert.match(credential, /home_dir\(\)\.join\("\.baijimu"\)\.join\("codex"\)\.join\("p"\)/);
  assert.match(credential, /Sha256::digest\(profile_id\.as_bytes\(\)\)/);
  assert.match(credential, /digest\[\.\.24\]\.to_string\(\)/);
  assert.match(credential, /migrate_legacy_profile_homes/);
  assert.match(credential, /fs::rename\(&source, &target\)/);
  assert.match(credential, /源目录和目标目录同时存在/);
  assert.match(credential, /a_new_profile_never_adopts_the_default_codex_home/);
  assert.match(credential, /v4_legacy_profile_directory_is_atomically_migrated_to_the_short_home/);
  assert.match(credential, /legacy_profile_migration_recovers_after_rename_before_metadata_save/);
  assert.match(credential, /legacy_profile_migration_preserves_both_directories_on_collision/);
  const migrationDetection = main.indexOf("credential::pending_profile_home_migration()");
  const desktopStop = main.indexOf("desktop::stop_for_codex_home_switch()", migrationDetection);
  const metadataMigration = main.indexOf("match credential::state()", desktopStop);
  assert.ok(migrationDetection >= 0);
  assert.ok(desktopStop > migrationDetection);
  assert.ok(metadataMigration > desktopStop);
  assert.match(main, /thread::spawn\(move \|\| match initialize_server\(\)/);
  assert.doesNotMatch(credential, /default_home_can_be_initialized/);
  assert.match(credential, /OWNERSHIP_MARKER_FILE: &str = "\.baijimu-owner\.json"/);
  assert.match(credential, /OWNERSHIP_OWNER: &str = "baijimu-connector-codex"/);
  assert.match(credential, /read_valid_ownership/);
  assert.match(credential, /commit_default_home_ownership/);
  assert.match(credential, /managed_files: vec!\[OWNED_AUTH_FILE\.to_string\(\), OWNED_CONFIG_FILE\.to_string\(\)\]/);
  assert.match(credential, /assert!\(!marker_content\.contains\("642"\)\)/);
  assert.match(credential, /assert!\(!marker_content\.contains\("workspace-token"\)\)/);
  assert.match(credential, /legacy_default_home_ownership_marker_contains_no_business_identifiers/);
});

test("all package identities agree with the GitHub source tag", async () => {
  const cargo = await readFile(join(root, "Cargo.toml"), "utf8");
  const cargoLock = await readFile(join(root, "Cargo.lock"), "utf8");
  const packageJson = JSON.parse(await readFile(join(root, "package.json"), "utf8"));
  const packageLock = JSON.parse(await readFile(join(root, "package-lock.json"), "utf8"));
  const nodeLauncher = await readFile(
    join(root, "bin", "baijimu-connector-codex.js"),
    "utf8",
  );
  const manifest = JSON.parse(await readFile(join(root, "connector.json"), "utf8"));
  const version = cargo.match(/^version = "([^"]+)"$/m)?.[1];
  assert.ok(version);
  assert.equal(cargoLock.match(/^name = "baijimu-connector-codex"\nversion = "([^"]+)"$/m)?.[1], version);
  assert.equal(packageJson.version, version);
  assert.equal(packageLock.version, version);
  assert.equal(packageLock.packages[""].version, version);
  assert.match(nodeLauncher, new RegExp(`const VERSION = "${version.replaceAll(".", "\\.")}";`));
  assert.equal(manifest.version, version);
  assert.deepEqual(manifest.source, {
    type: "github",
    repo: "momoplan/baijimu-connector-codex",
    revision: `v${version}`,
  });
});
