#!/usr/bin/env bash
set -Eeuo pipefail
export PATH="$HOME/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin"

CODEX_MODEL="${CODEX_MODEL:-gpt-5.6-sol}"
case "$CODEX_MODEL" in
  *[!A-Za-z0-9._-]*|"") echo "invalid CODEX_MODEL: $CODEX_MODEL" >&2; exit 1 ;;
esac
BAIJIMU_WORKSPACE_ID="${CODEX_WORKSPACE_ID:-${BAIJIMU_WORKSPACE_ID:-${WORKSPACE_ID:-}}}"
BAIJIMU_PROJECT_ID="${CODEX_PROJECT_ID:-${BAIJIMU_PROJECT_ID:-${PROJECT_ID:-}}}"
BAIJIMU_AGENT_CONFIG_ID="${CODEX_AGENT_CONFIG_ID:-${BAIJIMU_AGENT_CONFIG_ID:-}}"
BAIJIMU_AGENT_SESSION_ID="${CODEX_AGENT_SESSION_ID:-${BAIJIMU_AGENT_SESSION_ID:-}}"
BAIJIMU_SESSION_ID="${CODEX_SESSION_ID:-${BAIJIMU_SESSION_ID:-${SESSION_ID:-}}}"
ROUTER_BASE_URL="${CODEX_ROUTER_BASE_URL:-https://router.baijimu.com/api/claudecode/v1}"
case "$BAIJIMU_WORKSPACE_ID" in *[!0-9]*|"") echo "CODEX_WORKSPACE_ID or BAIJIMU_WORKSPACE_ID is required" >&2; exit 1 ;; esac
case "$BAIJIMU_PROJECT_ID" in *[!0-9]*) echo "invalid CODEX_PROJECT_ID or BAIJIMU_PROJECT_ID" >&2; exit 1 ;; esac

started_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
start_epoch="$(date +%s)"
state_dir="${CODEX_INSTALL_STATE_DIR:-${TMPDIR:-/tmp}/baijimu-codex-install}"
status_path="$state_dir/status.json"
result_path="$state_dir/result.json"
codex_dir="${CODEX_HOME:-$HOME/.codex}"
mkdir -p "$state_dir"

manifest_url="https://download.baijimu.com/codex-artifacts/latest.json"
target_app_path="/Applications/ChatGPT.app"
legacy_app_path="/Applications/Codex.app"
cli_path=""
app_path=""
app_install_method=""
app_version=""
app_bundle_id=""
cli_install_method=""
cli_version=""
cli_smoke=false
router_status=""
process_count=0
visible_window=false
config_written=false
auth_written=false
shared_cli_token_read=false
llm_credential_created=false

current_step=0
step_count=11
step1_name="Check ChatGPT desktop app"; step1_state="pending"; step1_detail=""; step1_downloaded=""; step1_total=""
step2_name="Read App package manifest"; step2_state="pending"; step2_detail=""; step2_downloaded=""; step2_total=""
step3_name="Download ChatGPT desktop app"; step3_state="pending"; step3_detail=""; step3_downloaded=""; step3_total=""
step4_name="Verify and install App"; step4_state="pending"; step4_detail=""; step4_downloaded=""; step4_total=""
step5_name="Install Codex CLI"; step5_state="pending"; step5_detail=""; step5_downloaded=""; step5_total=""
step6_name="Create Baijimu LLM credential and config"; step6_state="pending"; step6_detail=""; step6_downloaded=""; step6_total=""
step7_name="Verify Baijimu router"; step7_state="pending"; step7_detail=""; step7_downloaded=""; step7_total=""
step8_name="Verify Codex CLI"; step8_state="pending"; step8_detail=""; step8_downloaded=""; step8_total=""
step9_name="Restart ChatGPT desktop app"; step9_state="pending"; step9_detail=""; step9_downloaded=""; step9_total=""
step10_name="Verify process"; step10_state="pending"; step10_detail=""; step10_downloaded=""; step10_total=""
step11_name="Verify visible window"; step11_state="pending"; step11_detail=""; step11_downloaded=""; step11_total=""

json_escape() {
  printf '%s' "${1:-}" | awk 'BEGIN { ORS = ""; first = 1 } {
    if (!first) { printf "\\n" }
    first = 0
    gsub(/\\/, "\\\\")
    gsub(/"/, "\\\"")
    gsub(/\r/, "\\r")
    gsub(/\t/, "\\t")
    printf "%s", $0
  }'
}

json_string() { printf '"'; json_escape "${1:-}"; printf '"'; }

json_number_or_null() {
  case "${1:-}" in
    ""|*[!0-9]*) printf 'null' ;;
    *) printf '%s' "$1" ;;
  esac
}

write_install_console() {
  [ "${CODEX_INSTALL_QUIET:-}" = "1" ] && return 0
  printf '%s\n' "$*" >&2
}

write_status() {
  {
    printf '{\n'
    printf '  "title": "Baijimu is installing ChatGPT desktop app and Codex",\n'
    printf '  "platform": "macos",\n'
    printf '  "startedAt": '; json_string "$started_at"; printf ',\n'
    printf '  "updatedAt": '; json_string "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"; printf ',\n'
    printf '  "currentStep": %s,\n' "$current_step"
    printf '  "statusPath": '; json_string "$status_path"; printf ',\n'
    printf '  "resultPath": '; json_string "$result_path"; printf ',\n'
    printf '  "steps": [\n'
    for index in 1 2 3 4 5 6 7 8 9 10 11; do
      eval "name=\${step${index}_name}"
      eval "state=\${step${index}_state}"
      eval "detail=\${step${index}_detail}"
      eval "downloaded=\${step${index}_downloaded}"
      eval "total=\${step${index}_total}"
      printf '    { "index": %s, "name": ' "$index"; json_string "$name"; printf ', "state": '; json_string "$state"
      printf ', "detail": '; json_string "$detail"; printf ', "downloadedBytes": '; json_number_or_null "$downloaded"
      printf ', "totalBytes": '; json_number_or_null "$total"; printf ' }'
      [ "$index" -lt "$step_count" ] && printf ','
      printf '\n'
    done
    printf '  ]\n'
    printf '}\n'
  } > "$status_path"
}

set_step() {
  index="$1"
  state="$2"
  detail="${3:-}"
  downloaded="${4:-}"
  total="${5:-}"
  current_step="$index"
  eval "step${index}_state=\$state"
  eval "step${index}_detail=\$detail"
  eval "step${index}_downloaded=\$downloaded"
  eval "step${index}_total=\$total"
  write_status

  eval "name=\${step${index}_name}"
  label="[$index/$step_count] $name"
  if [ -n "$downloaded" ] && [ -n "$total" ] && [ "$total" -gt 0 ] 2>/dev/null; then
    downloaded_mb="$(awk -v bytes="$downloaded" 'BEGIN { printf "%.1f", bytes / 1024 / 1024 }')"
    total_mb="$(awk -v bytes="$total" 'BEGIN { printf "%.1f", bytes / 1024 / 1024 }')"
    write_install_console "$label  $state  ${downloaded_mb}MB / ${total_mb}MB"
  elif [ -n "$detail" ]; then
    write_install_console "$label  $state  $detail"
  else
    write_install_console "$label  $state"
  fi
}

complete_pending_steps() {
  state="$1"
  detail="$2"
  for index in 1 2 3 4 5 6 7 8 9 10 11; do
    eval "step_state=\${step${index}_state}"
    if [ "$step_state" = "pending" ]; then
      eval "step${index}_state=\$state"
      eval "step${index}_detail=\$detail"
    fi
  done
  write_status
}

finish_result() {
  ok="$1"
  error_message="${2:-}"
  elapsed_ms="$(( ($(date +%s) - start_epoch) * 1000 ))"
  {
    printf '{\n'
    printf '  "ok": %s,\n' "$ok"
    printf '  "platform": "macos",\n'
    printf '  "startedAt": '; json_string "$started_at"; printf ',\n'
    printf '  "codexHome": '; json_string "$codex_dir"; printf ',\n'
    printf '  "appInstalled": %s,\n' "$( [ -n "$app_path" ] && printf true || printf false )"
    printf '  "appInstallMethod": '; json_string "$app_install_method"; printf ',\n'
    printf '  "appPath": '; json_string "$app_path"; printf ',\n'
    printf '  "version": '; json_string "$app_version"; printf ',\n'
    printf '  "bundleId": '; json_string "$app_bundle_id"; printf ',\n'
    printf '  "cliInstalled": %s,\n' "$( [ -n "$cli_path" ] && printf true || printf false )"
    printf '  "cliInstallMethod": '; json_string "$cli_install_method"; printf ',\n'
    printf '  "cliPath": '; json_string "$cli_path"; printf ',\n'
    printf '  "workspaceId": %s,\n' "$BAIJIMU_WORKSPACE_ID"
    printf '  "projectId": '; json_number_or_null "$BAIJIMU_PROJECT_ID"; printf ',\n'
    printf '  "sharedCliTokenRead": %s,\n' "$shared_cli_token_read"
    printf '  "llmCredentialCreated": %s,\n' "$llm_credential_created"
    printf '  "configWritten": %s,\n' "$config_written"
    printf '  "authWritten": %s,\n' "$auth_written"
    printf '  "routerHttpStatus": '; [ -n "$router_status" ] && printf '%s' "$router_status" || printf 'null'; printf ',\n'
    printf '  "cliVersion": '; json_string "$cli_version"; printf ',\n'
    printf '  "cliSmoke": %s,\n' "$cli_smoke"
    printf '  "appStarted": %s,\n' "$( [ "$process_count" -gt 0 ] 2>/dev/null && printf true || printf false )"
    printf '  "visibleWindow": %s,\n' "$visible_window"
    printf '  "processCount": %s,\n' "$process_count"
    printf '  "model": '; json_string "$CODEX_MODEL"; printf ',\n'
    printf '  "elapsedMs": %s,\n' "$elapsed_ms"
    if [ -n "$error_message" ]; then
      printf '  "warnings": [],\n'
      printf '  "errors": [ '; json_string "$error_message"; printf ' ]\n'
    else
      printf '  "warnings": [],\n'
      printf '  "errors": []\n'
    fi
    printf '}\n'
  } > "$result_path"
  cat "$result_path"
}

fail_install() {
  message="$1"
  trap - ERR
  if [ "$current_step" -gt 0 ]; then
    set_step "$current_step" "failed" "$message" || true
  fi
  complete_pending_steps "skipped" "Install stopped" || true
  write_install_console ""
  write_install_console "ChatGPT desktop app and Codex setup failed. Please send the error to Baijimu."
  finish_result false "$message"
  exit 1
}

trap 'fail_install "unexpected error at line $LINENO"' ERR

download_with_progress() {
  url="$1"
  output="$2"
  step="$3"
  label="$4"
  total="${5:-}"
  error_file="$state_dir/download-step-${step}.err"
  rm -f "$output"
  rm -f "$error_file"
  : > "$output"
  set_step "$step" "running" "$label" "" "$total"
  curl -fL --silent --show-error \
    --retry 5 --retry-all-errors --retry-delay 2 \
    --connect-timeout 15 --max-time 900 \
    "$url" -o "$output" 2> "$error_file" &
  curl_pid="$!"
  while kill -0 "$curl_pid" 2>/dev/null; do
    size="$(stat -f '%z' "$output" 2>/dev/null || printf '0')"
    set_step "$step" "running" "$label" "$size" "$total"
    sleep 1
  done
  curl_status=0
  wait "$curl_pid" || curl_status=$?
  if [ "$curl_status" -ne 0 ]; then
    error_detail="$(tail -n 5 "$error_file" 2>/dev/null | tr '\n' ' ' | sed 's/[[:space:]]*$//')"
    rm -f "$error_file"
    [ -n "$error_detail" ] || error_detail="curl returned no diagnostic output"
    fail_install "$label failed from $url (curl exit $curl_status): $error_detail"
  fi
  rm -f "$error_file"
  size="$(stat -f '%z' "$output" 2>/dev/null || printf '0')"
  set_step "$step" "completed" "$label" "$size" "$total"
}

installed_app_path() {
  if [ -d "$target_app_path" ]; then
    printf '%s\n' "$target_app_path"
  elif [ -d "$legacy_app_path" ]; then
    printf '%s\n' "$legacy_app_path"
  fi
}

read_app_metadata_value() {
  bundle_path="$1"
  metadata_key="$2"
  plist_key="$3"
  value="$(mdls -raw -name "$metadata_key" "$bundle_path" 2>/dev/null || true)"
  if [ -z "$value" ] || [ "$value" = "(null)" ]; then
    value="$(defaults read "$bundle_path/Contents/Info" "$plist_key" 2>/dev/null || true)"
  fi
  printf '%s\n' "$value"
}

refresh_app_metadata() {
  app_version="$(read_app_metadata_value "$app_path" kMDItemVersion CFBundleShortVersionString)"
  app_bundle_id="$(read_app_metadata_value "$app_path" kMDItemCFBundleIdentifier CFBundleIdentifier)"
}

asset_fields() {
  manifest="$1"
  asset="$2"
  asset_block="$(awk -v name="$asset" '
    $0 ~ "\"name\"[[:space:]]*:[[:space:]]*\"" name "\"" { found = 1 }
    found { print }
    found && $0 ~ /^[[:space:]]*}[,]?[[:space:]]*$/ { exit }
  ' "$manifest")"
  mirror_url="$(printf '%s\n' "$asset_block" | awk -F'"' '/"mirror_url"[[:space:]]*:/ { print $4; exit }')"
  sha256="$(printf '%s\n' "$asset_block" | awk -F'"' '/"sha256"[[:space:]]*:/ { print $4; exit }')"
  size_bytes="$(printf '%s\n' "$asset_block" | awk -F'[: ,]+' '/"size_bytes"[[:space:]]*:/ { gsub(/[^0-9]/, "", $3); print $3; exit }')"
  [ -z "$size_bytes" ] && size_bytes="$(printf '%s\n' "$asset_block" | awk -F'[: ,]+' '/"size"[[:space:]]*:/ { gsub(/[^0-9]/, "", $3); print $3; exit }')"
  if [ -z "$mirror_url" ] || [ -z "$sha256" ]; then
    return 1
  fi
}

ensure_codex_app() {
  set_step 1 "running" "Checking ChatGPT desktop app"
  existing_app_path="$(installed_app_path)"
  if [ -n "$existing_app_path" ]; then
    app_path="$existing_app_path"
    app_install_method="already-installed"
    refresh_app_metadata
    set_step 1 "completed" "ChatGPT desktop app is already installed"
    set_step 2 "skipped" "App package download is not needed"
    set_step 3 "skipped" "App package download is not needed"
    set_step 4 "skipped" "App reinstall is not needed"
    return
  fi

  set_step 1 "completed" "ChatGPT desktop app is not installed; preparing install"
  arch="$(uname -m)"
  case "$arch" in
    arm64) app_asset="codex-app-aarch64-apple-darwin.dmg" ;;
    x86_64) app_asset="codex-app-x86_64-apple-darwin.dmg" ;;
    *) fail_install "Baijimu cache does not include this macOS architecture: $arch" ;;
  esac

  work_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-app.XXXXXX")"
  mount_dir=""
  manifest="$work_dir/latest.json"
  dmg="$work_dir/$app_asset"
  download_with_progress "$manifest_url" "$manifest" 2 "Reading Baijimu package manifest" ""
  if ! asset_fields "$manifest" "$app_asset"; then
    rm -rf "$work_dir"
    fail_install "Baijimu cache asset is missing or incomplete: $app_asset"
  fi
  set_step 2 "completed" "Found $app_asset"
  download_with_progress "$mirror_url" "$dmg" 3 "Downloading official ChatGPT desktop app package" "$size_bytes"

  set_step 4 "running" "Verifying App package SHA256"
  actual="$(shasum -a 256 "$dmg" | awk '{print $1}')"
  if [ "$actual" != "$sha256" ]; then
    rm -rf "$work_dir"
    fail_install "SHA256 mismatch for $app_asset"
  fi

  set_step 4 "running" "Mounting and installing ChatGPT desktop app"
  mount_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-dmg.XXXXXX")"
  hdiutil attach "$dmg" -mountpoint "$mount_dir" -nobrowse -quiet
  if [ -d "$mount_dir/ChatGPT.app" ]; then
    source_app="$mount_dir/ChatGPT.app"
  elif [ -d "$mount_dir/Codex.app" ]; then
    source_app="$mount_dir/Codex.app"
  else
    source_app="$(find "$mount_dir" -maxdepth 1 -name '*.app' -type d | head -n 1)"
  fi
  if [ -z "${source_app:-}" ] || [ ! -d "$source_app" ]; then
    hdiutil detach "$mount_dir" -quiet || true
    rm -rf "$work_dir"
    fail_install "No supported app bundle found in DMG"
  fi
  app_path="/Applications/$(basename "$source_app")"
  ditto "$source_app" "$app_path"
  hdiutil detach "$mount_dir" -quiet
  mount_dir=""
  rm -rf "$work_dir"
  xattr -dr com.apple.quarantine "$app_path" 2>/dev/null || true
  test -d "$app_path"
  app_install_method="baijimu-cache-dmg"
  refresh_app_metadata
  set_step 4 "completed" "ChatGPT desktop app installed"
}

resolve_codex_cli() {
  local candidate
  candidate="$(command -v codex 2>/dev/null || true)"
  if [ -n "$candidate" ] && [ -x "$candidate" ]; then
    printf '%s\n' "$candidate"
    return 0
  fi
  if [ -x "$HOME/.local/bin/codex" ]; then
    printf '%s\n' "$HOME/.local/bin/codex"
    return 0
  fi
  return 1
}

install_codex_cli_from_cache() {
  local install_target install_temp
  arch="$(uname -m)"
  case "$arch" in
    arm64) cli_asset="codex-aarch64-apple-darwin.tar.gz" ;;
    x86_64) cli_asset="codex-x86_64-apple-darwin.tar.gz" ;;
    *) fail_install "Baijimu cache does not include Codex CLI for this macOS architecture: $arch" ;;
  esac

  work_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-cli.XXXXXX")"
  manifest="$work_dir/latest.json"
  archive="$work_dir/$cli_asset"
  download_with_progress "$manifest_url" "$manifest" 5 "Reading Baijimu CLI package manifest" ""
  if ! asset_fields "$manifest" "$cli_asset"; then
    rm -rf "$work_dir"
    fail_install "Baijimu cache asset is missing or incomplete: $cli_asset"
  fi
  download_with_progress "$mirror_url" "$archive" 5 "Downloading official Codex CLI package" "$size_bytes"

  set_step 5 "running" "Verifying Codex CLI package SHA256"
  actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
  if [ "$actual" != "$sha256" ]; then
    rm -rf "$work_dir"
    fail_install "SHA256 mismatch for $cli_asset"
  fi

  set_step 5 "running" "Installing Codex CLI"
  tar -xzf "$archive" -C "$work_dir"
  bin="$(find "$work_dir" -maxdepth 4 -type f \( -name codex -o -name 'codex-*' \) ! -name '*.tar.gz' -perm -111 2>/dev/null | head -n 1)"
  if [ -z "${bin:-}" ]; then
    bin="$(find "$work_dir" -maxdepth 4 -type f \( -name codex -o -name 'codex-*' \) ! -name '*.tar.gz' 2>/dev/null | head -n 1)"
  fi
  if [ -z "${bin:-}" ]; then
    rm -rf "$work_dir"
    fail_install "codex binary not found after extracting $cli_asset"
  fi
  mkdir -p "$HOME/.local/bin"
  install_target="$HOME/.local/bin/codex"
  install_temp="$HOME/.local/bin/.codex.install.$$"
  rm -f "$install_temp"
  install -m 755 "$bin" "$install_temp"
  mv -f "$install_temp" "$install_target"
  xattr -d com.apple.quarantine "$install_target" 2>/dev/null || true
  profile="$HOME/.zshrc"
  line='export PATH="$HOME/.local/bin:$PATH"'
  if [ ! -f "$profile" ] || ! grep -Fq '.local/bin' "$profile"; then
    printf '\n# Added by Baijimu Codex installer\n%s\n' "$line" >> "$profile"
  fi
  rm -rf "$work_dir"
  cli_install_method="baijimu-cache-tar"
  cli_path="$install_target"
}

ensure_codex_cli() {
  set_step 5 "running" "Checking Codex CLI"
  if cli_path="$(resolve_codex_cli)"; then
    cli_install_method="already-installed"
    set_step 5 "completed" "Codex CLI is already available"
    return
  fi
  install_codex_cli_from_cache
  if ! cli_path="$(resolve_codex_cli)"; then
    fail_install "codex CLI not found after installation"
  fi
  set_step 5 "completed" "Codex CLI installed"
}

shared_auth_path() {
  if [ -n "${BAIJIMU_CONFIG_HOME:-}" ]; then
    printf '%s\n' "$BAIJIMU_CONFIG_HOME/baijimu/auth.json"
  else
    printf '%s\n' "$HOME/.config/baijimu/auth.json"
  fi
}

extract_llm_credential_from_json_file() {
  output_file="$1"
  osascript -l JavaScript - "$output_file" <<'JS'
ObjC.import("Foundation");

function run(argv) {
  const path = argv[0];
  const data = $.NSData.dataWithContentsOfFile(path);
  if (!data) {
    return "";
  }
  const document = ObjC.deepUnwrap($.NSJSONSerialization.JSONObjectWithDataOptionsError(data, 0, null));
  const payload = document && document.data ? document.data : document;
  const credential = payload.llmCredential || payload.credential || payload.apiKey || "";
  if (typeof credential === "string") {
    return credential;
  }
  return "";
}
JS
}

create_baijimu_llm_credential() {
  if [ -n "${CODEX_LLM_CREDENTIAL_FILE:-}" ]; then
    [ -f "$CODEX_LLM_CREDENTIAL_FILE" ] || return 1
    credential_mode="$(stat -f '%Lp' "$CODEX_LLM_CREDENTIAL_FILE" 2>/dev/null || stat -c '%a' "$CODEX_LLM_CREDENTIAL_FILE" 2>/dev/null || true)"
    case "$credential_mode" in
      600|400) ;;
      *) return 1 ;;
    esac
    credential="$(tr -d '\r\n' < "$CODEX_LLM_CREDENTIAL_FILE")"
    [ -n "$credential" ] || return 1
    llm_credential_created=true
    printf '%s\n' "$credential"
    return 0
  fi
  command -v "${BAIJIMU_CLI_BIN:-baijimu}" >/dev/null 2>&1 || return 1
  output_file="$(mktemp "${TMPDIR:-/tmp}/baijimu-llm-credential.XXXXXX")"
  error_file="$state_dir/baijimu-llm-credential.err"
  chmod 600 "$output_file"

  cmd=("${BAIJIMU_CLI_BIN:-baijimu}" --json llm-credential create
    --workspace-id "$BAIJIMU_WORKSPACE_ID"
    --show-secret)
  if [ -n "$BAIJIMU_PROJECT_ID" ]; then
    cmd+=(--project-id "$BAIJIMU_PROJECT_ID")
  fi
  if [ -n "${BAIJIMU_AGENT_CONFIG_ID:-}" ]; then
    cmd+=(--agent-config-id "$BAIJIMU_AGENT_CONFIG_ID")
  fi
  if [ -n "${BAIJIMU_AGENT_SESSION_ID:-}" ]; then
    cmd+=(--agent-session-id "$BAIJIMU_AGENT_SESSION_ID")
  fi
  if [ -n "${BAIJIMU_SESSION_ID:-}" ]; then
    cmd+=(--session-id "$BAIJIMU_SESSION_ID")
  fi

  if ! "${cmd[@]}" > "$output_file" 2> "$error_file"; then
    rm -f "$output_file"
    return 1
  fi
  credential="$(extract_llm_credential_from_json_file "$output_file")"
  rm -f "$output_file"
  [ -n "$credential" ] || return 1
  printf '%s\n' "$credential"
}

escape_string() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

backup_if_exists() {
  path="$1"
  [ -e "$path" ] || return 0
  cp -p "$path" "$path.bak.$(date +%s)"
}

remove_managed_codex_block() {
  awk '
    $0 == "# >>> baijimu managed codex router" { skipping = 1; next }
    skipping && $0 == "# <<< baijimu managed codex router" { skipping = 0; next }
    !skipping { print }
  ' "$1"
}

remove_toml_table() {
  table_name="$1"
  awk -v table_name="$table_name" '
    function trim(value) {
      sub(/^[[:space:]]+/, "", value)
      sub(/[[:space:]]+$/, "", value)
      return value
    }
    trim($0) == table_name { skipping = 1; next }
    skipping && trim($0) ~ /^\[.+\]$/ { skipping = 0 }
    !skipping { print }
  '
}

remove_root_codex_keys() {
  awk '
    /^[[:space:]]*\[.+\][[:space:]]*$/ { in_table = 1 }
    !in_table && /^[[:space:]]*(model_provider|model|sandbox_mode|approval_policy|cli_auth_credentials_store|forced_login_method)[[:space:]]*=/ { next }
    { print }
  '
}

write_codex_config() {
  api_key="$1"
  auth_file="$codex_dir/auth.json"
  config_file="$codex_dir/config.toml"
  work_dir="$(mktemp -d "${TMPDIR:-/tmp}/baijimu-codex-config.XXXXXX")"

  mkdir -p "$codex_dir"
  chmod 700 "$codex_dir"
  backup_if_exists "$auth_file"
  backup_if_exists "$config_file"

  umask 077
  {
    printf '{\n'
    printf '  "OPENAI_API_KEY": "%s",\n' "$(escape_string "$api_key")"
    printf '  "auth_mode": "apikey"\n'
    printf '}\n'
  } > "$work_dir/auth.json"
  mv "$work_dir/auth.json" "$auth_file"
  chmod 600 "$auth_file"

  existing_config="$work_dir/existing-config.toml"
  managed_config="$work_dir/managed-config.toml"
  preserved_config="$work_dir/preserved-config.toml"
  if [ -f "$config_file" ]; then
    cp "$config_file" "$existing_config"
  else
    : > "$existing_config"
  fi
  remove_managed_codex_block "$existing_config" \
    | remove_toml_table "[model_providers.baijimu-router]" \
    | remove_root_codex_keys \
    | sed '/^[[:space:]]*$/N;/^\n$/D' > "$preserved_config"

  {
    printf '# >>> baijimu managed codex router\n'
    printf 'model_provider = "baijimu-router"\n'
    printf 'model = "%s"\n' "$(escape_string "$CODEX_MODEL")"
    printf 'sandbox_mode = "danger-full-access"\n'
    printf 'approval_policy = "on-request"\n'
    printf 'cli_auth_credentials_store = "file"\n'
    printf 'forced_login_method = "api"\n'
    printf '\n'
    printf '[model_providers.baijimu-router]\n'
    printf 'name = "baijimu-router"\n'
    printf 'base_url = "%s"\n' "$(escape_string "$ROUTER_BASE_URL")"
    printf 'wire_api = "responses"\n'
    printf 'requires_openai_auth = true\n'
    printf '# <<< baijimu managed codex router\n'
  } > "$managed_config"

  if [ -s "$preserved_config" ]; then
    cat "$managed_config" > "$work_dir/config.toml"
    printf '\n' >> "$work_dir/config.toml"
    cat "$preserved_config" >> "$work_dir/config.toml"
  else
    cat "$managed_config" > "$work_dir/config.toml"
  fi
  mv "$work_dir/config.toml" "$config_file"
  chmod 600 "$config_file"
  rm -rf "$work_dir"
}

configure_codex_terminal() {
  set_step 6 "running" "Creating Baijimu LLM credential and writing Codex config"
  if ! local_api_key="$(create_baijimu_llm_credential)"; then
    error_detail="$(tail -n 5 "$state_dir/baijimu-llm-credential.err" 2>/dev/null | tr '\n' ' ')"
    fail_install "failed to create Baijimu LLM credential for workspace $BAIJIMU_WORKSPACE_ID: $error_detail"
  fi
  if [ -z "$local_api_key" ]; then
    fail_install "Baijimu CLI did not return an LLM credential for workspace $BAIJIMU_WORKSPACE_ID"
  fi
  shared_cli_token_read=true
  llm_credential_created=true
  write_codex_config "$local_api_key"
  config_written=true
  auth_written=true
  test "$(stat -f '%Lp' "$codex_dir/auth.json")" = "600"
  set_step 6 "completed" "Codex config written from Baijimu LLM credential"
}

verify_router() {
  set_step 7 "running" "Verifying Baijimu router"
  router_err="$state_dir/codex-router.err"
  if ! router_status="$(
    curl -sS -m 60 -o /tmp/codex-router-responses.json -w '%{http_code}' \
      -H "Authorization: Bearer $local_api_key" \
      -H 'Content-Type: application/json' \
      -d "{\"model\":\"$CODEX_MODEL\",\"input\":\"Reply with exactly OK\"}" \
      "$ROUTER_BASE_URL/responses" 2> "$router_err"
  )"; then
    error_detail="$(tail -n 5 "$router_err" 2>/dev/null | tr '\n' ' ')"
    rm -f /tmp/codex-router-responses.json "$router_err"
    unset local_api_key
    fail_install "router /responses health check failed: $error_detail"
  fi
  rm -f /tmp/codex-router-responses.json "$router_err"
  if [ "$router_status" != "200" ]; then
    unset local_api_key
    fail_install "router /responses health check failed: HTTP $router_status"
  fi
  set_step 7 "completed" "Baijimu router verified"
}

verify_codex_cli() {
  set_step 8 "running" "Checking Codex CLI version"
  cli_version="$("$cli_path" --version 2>&1)"
  if smoke_output="$("$cli_path" exec --skip-git-repo-check "Reply exactly OK" 2>&1)"; then
    smoke_exit_code=0
  else
    smoke_exit_code=$?
  fi
  if [ "$smoke_exit_code" -ne 0 ]; then
    smoke_detail="$(awk 'NF { line = $0 } END { print line }' <<< "$smoke_output")"
    fail_install "codex CLI smoke test failed with exit code $smoke_exit_code: $smoke_detail"
  fi

  smoke_ok=false
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%$'\r'}"
    if [ "$line" = "OK" ]; then
      smoke_ok=true
      break
    fi
  done <<< "$smoke_output"
  if [ "$smoke_ok" != true ]; then
    fail_install "codex CLI smoke test completed without an exact OK response"
  fi
  cli_smoke=true
  unset smoke_output
  set_step 8 "completed" "Codex CLI verified"
}

verify_codex_window() {
  if [ "${CODEX_INSTALL_SKIP_DESKTOP_RESTART:-}" = "1" ]; then
    set_step 9 "skipped" "Existing ChatGPT desktop session was preserved"
    set_step 10 "skipped" "Desktop process restart was not requested"
    set_step 11 "skipped" "Isolated Codex profile was verified through the CLI"
    return
  fi
  set_step 9 "running" "Restarting ChatGPT desktop app"
  if [ -z "$app_path" ] || [ ! -d "$app_path" ]; then
    fail_install "ChatGPT desktop app is not installed"
  fi
  refresh_app_metadata
  if [ -z "$app_bundle_id" ] || [ "$app_bundle_id" = "(null)" ]; then
    fail_install "ChatGPT desktop app bundle identifier check failed"
  fi
  pkill -f "$app_path/Contents/MacOS" 2>/dev/null || true
  pkill -f "$app_path/Contents/Resources/codex app-server" 2>/dev/null || true
  pkill -f "$app_path/Contents/Frameworks" 2>/dev/null || true
  sleep 3
  open "$app_path"
  sleep 6
  set_step 9 "completed" "ChatGPT desktop app opened"

  set_step 10 "running" "Checking ChatGPT desktop app process"
  process_output="$(pgrep -fl "$app_path/Contents/MacOS|codex app-server" || true)"
  process_count="$(printf '%s\n' "$process_output" | awk 'NF { count++ } END { print count + 0 }')"
  if [ "$process_count" -lt 1 ]; then
    fail_install "ChatGPT desktop app process was not found"
  fi
  set_step 10 "completed" "ChatGPT desktop app process is running"

  set_step 11 "running" "Checking visible ChatGPT Codex window"
  info="$(/usr/bin/lsappinfo info -only pid,front,visible,windows "$app_bundle_id" 2>&1 || true)"
  if [[ "$info" == *'windows=[ NULL ]'* ]]; then
    project="$HOME/Documents/Codex/$(date +%F)/default"
    mkdir -p "$project"
    open -a "$app_path" "$project" || true
    osascript -e "tell application id \"$app_bundle_id\" to activate" \
              -e "tell application id \"$app_bundle_id\" to reopen" 2>/dev/null || true
    sleep 5
    info="$(/usr/bin/lsappinfo info -only pid,front,visible,windows "$app_bundle_id" 2>&1 || true)"
  fi
  if [[ "$info" == *'windows=[ NULL ]'* ]]; then
    fail_install "ChatGPT desktop app started without a visible window"
  fi
  visible_window=true
  set_step 11 "completed" "ChatGPT Codex window is visible"
}

write_install_console ""
write_install_console "Baijimu is installing ChatGPT desktop app and Codex"
write_install_console "Please keep this window open."
write_install_console ""
write_status

ensure_codex_app
ensure_codex_cli
configure_codex_terminal
verify_router
unset local_api_key
verify_codex_cli
verify_codex_window

trap - ERR
complete_pending_steps "skipped" "Install completed"
write_install_console ""
write_install_console "ChatGPT desktop app and Codex setup completed. You can close this window."
finish_result true ""
