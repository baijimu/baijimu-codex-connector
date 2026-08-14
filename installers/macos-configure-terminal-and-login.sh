#!/usr/bin/env bash
set -Eeuo pipefail
export PATH="$HOME/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin"

CODEX_MODEL="${CODEX_MODEL:-gpt-5.6-sol}"
case "$CODEX_MODEL" in
  *[!A-Za-z0-9._-]*|"") echo "CODEX_MODEL 无效：$CODEX_MODEL" >&2; exit 1 ;;
esac
BAIJIMU_WORKSPACE_ID="${CODEX_WORKSPACE_ID:-${BAIJIMU_WORKSPACE_ID:-${WORKSPACE_ID:-}}}"
BAIJIMU_PROJECT_ID="${CODEX_PROJECT_ID:-${BAIJIMU_PROJECT_ID:-${PROJECT_ID:-}}}"
BAIJIMU_AGENT_CONFIG_ID="${CODEX_AGENT_CONFIG_ID:-${BAIJIMU_AGENT_CONFIG_ID:-}}"
BAIJIMU_AGENT_SESSION_ID="${CODEX_AGENT_SESSION_ID:-${BAIJIMU_AGENT_SESSION_ID:-}}"
BAIJIMU_SESSION_ID="${CODEX_SESSION_ID:-${BAIJIMU_SESSION_ID:-${SESSION_ID:-}}}"
ROUTER_BASE_URL="${CODEX_ROUTER_BASE_URL:-https://router.baijimu.com/api/claudecode/v1}"
case "$BAIJIMU_WORKSPACE_ID" in *[!0-9]*|"") echo "必须提供 CODEX_WORKSPACE_ID 或 BAIJIMU_WORKSPACE_ID" >&2; exit 1 ;; esac
case "$BAIJIMU_PROJECT_ID" in *[!0-9]*) echo "CODEX_PROJECT_ID 或 BAIJIMU_PROJECT_ID 无效" >&2; exit 1 ;; esac

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
config_written=false
auth_written=false
shared_cli_token_read=false
llm_credential_created=false

current_step=0
step_count=9
step1_name="检查 ChatGPT 桌面应用"; step1_state="pending"; step1_detail=""; step1_downloaded=""; step1_total=""
step2_name="读取应用安装包清单"; step2_state="pending"; step2_detail=""; step2_downloaded=""; step2_total=""
step3_name="下载 ChatGPT 桌面应用"; step3_state="pending"; step3_detail=""; step3_downloaded=""; step3_total=""
step4_name="校验并安装应用"; step4_state="pending"; step4_detail=""; step4_downloaded=""; step4_total=""
step5_name="安装 Codex CLI"; step5_state="pending"; step5_detail=""; step5_downloaded=""; step5_total=""
step6_name="创建百积木 LLM 凭证和配置"; step6_state="pending"; step6_detail=""; step6_downloaded=""; step6_total=""
step7_name="验证百积木路由"; step7_state="pending"; step7_detail=""; step7_downloaded=""; step7_total=""
step8_name="验证 Codex CLI"; step8_state="pending"; step8_detail=""; step8_downloaded=""; step8_total=""
step9_name="完成安装配置"; step9_state="pending"; step9_detail=""; step9_downloaded=""; step9_total=""

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
  local index name state detail downloaded total status_temp
  status_temp="$(mktemp "$state_dir/status.json.XXXXXX")"
  {
    printf '{\n'
    printf '  "title": "百积木正在安装 ChatGPT 桌面应用和 Codex",\n'
    printf '  "locale": "zh-CN",\n'
    printf '  "platform": "macos",\n'
    printf '  "startedAt": '; json_string "$started_at"; printf ',\n'
    printf '  "updatedAt": '; json_string "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"; printf ',\n'
    printf '  "currentStep": %s,\n' "$current_step"
    printf '  "statusPath": '; json_string "$status_path"; printf ',\n'
    printf '  "resultPath": '; json_string "$result_path"; printf ',\n'
    printf '  "steps": [\n'
    for index in 1 2 3 4 5 6 7 8 9; do
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
  } > "$status_temp"
  mv -f "$status_temp" "$status_path"
}

set_step() {
  local index="$1"
  local state="$2"
  local detail="${3:-}"
  local downloaded="${4:-}"
  local total="${5:-}"
  local name console_label downloaded_mb total_mb
  current_step="$index"
  eval "step${index}_state=\$state"
  eval "step${index}_detail=\$detail"
  eval "step${index}_downloaded=\$downloaded"
  eval "step${index}_total=\$total"
  write_status

  eval "name=\${step${index}_name}"
  console_label="[$index/$step_count] $name"
  if [ -n "$downloaded" ] && [ -n "$total" ] && [ "$total" -gt 0 ] 2>/dev/null; then
    downloaded_mb="$(awk -v bytes="$downloaded" 'BEGIN { printf "%.1f", bytes / 1024 / 1024 }')"
    total_mb="$(awk -v bytes="$total" 'BEGIN { printf "%.1f", bytes / 1024 / 1024 }')"
    write_install_console "$console_label  $state  ${downloaded_mb}MB / ${total_mb}MB"
  elif [ -n "$detail" ]; then
    write_install_console "$console_label  $state  $detail"
  else
    write_install_console "$console_label  $state"
  fi
}

complete_pending_steps() {
  local state="$1"
  local detail="$2"
  local index step_state
  for index in 1 2 3 4 5 6 7 8 9; do
    eval "step_state=\${step${index}_state}"
    if [ "$step_state" = "pending" ]; then
      eval "step${index}_state=\$state"
      eval "step${index}_detail=\$detail"
    fi
  done
  write_status
}

finish_result() {
  local ok="$1"
  local error_message="${2:-}"
  local elapsed_ms="$(( ($(date +%s) - start_epoch) * 1000 ))"
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
  local message="$1"
  trap - ERR
  if [ "$current_step" -gt 0 ]; then
    set_step "$current_step" "failed" "$message" || true
  fi
  complete_pending_steps "skipped" "安装已停止" || true
  write_install_console ""
  write_install_console "ChatGPT 桌面应用和 Codex 配置失败，请将错误信息发送给百积木。"
  finish_result false "$message"
  exit 1
}

trap 'fail_install "第 $LINENO 行发生意外错误"' ERR

download_with_progress() {
  local url="$1"
  local output="$2"
  local step="$3"
  local detail="$4"
  local total="${5:-}"
  local error_file="$state_dir/download-step-${step}.err"
  local curl_pid size curl_status error_detail
  rm -f "$output"
  rm -f "$error_file"
  : > "$output"
  set_step "$step" "running" "$detail" "" "$total"
  curl -fL --silent --show-error \
    --retry 5 --retry-all-errors --retry-delay 2 \
    --connect-timeout 15 --max-time 900 \
    "$url" -o "$output" 2> "$error_file" &
  curl_pid="$!"
  while kill -0 "$curl_pid" 2>/dev/null; do
    size="$(stat -f '%z' "$output" 2>/dev/null || printf '0')"
    set_step "$step" "running" "$detail" "$size" "$total"
    sleep 1
  done
  curl_status=0
  wait "$curl_pid" || curl_status=$?
  if [ "$curl_status" -ne 0 ]; then
    error_detail="$(tail -n 5 "$error_file" 2>/dev/null | tr '\n' ' ' | sed 's/[[:space:]]*$//')"
    rm -f "$error_file"
    [ -n "$error_detail" ] || error_detail="curl 未返回诊断信息"
    fail_install "$detail 失败，下载地址：$url（curl 退出码 $curl_status）：$error_detail"
  fi
  rm -f "$error_file"
  size="$(stat -f '%z' "$output" 2>/dev/null || printf '0')"
  set_step "$step" "completed" "$detail" "$size" "$total"
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
  set_step 1 "running" "正在检查 ChatGPT 桌面应用"
  existing_app_path="$(installed_app_path)"
  if [ -n "$existing_app_path" ]; then
    app_path="$existing_app_path"
    app_install_method="already-installed"
    refresh_app_metadata
    set_step 1 "completed" "ChatGPT 桌面应用已安装"
    set_step 2 "skipped" "无需读取应用安装包清单"
    set_step 3 "skipped" "无需下载应用安装包"
    set_step 4 "skipped" "无需重新安装应用"
    return
  fi

  set_step 1 "completed" "未安装 ChatGPT 桌面应用，正在准备安装"
  arch="$(uname -m)"
  case "$arch" in
    arm64) app_asset="codex-app-aarch64-apple-darwin.dmg" ;;
    x86_64) app_asset="codex-app-x86_64-apple-darwin.dmg" ;;
    *) fail_install "百积木缓存不包含当前 macOS 架构：$arch" ;;
  esac

  work_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-app.XXXXXX")"
  mount_dir=""
  manifest="$work_dir/latest.json"
  dmg="$work_dir/$app_asset"
  download_with_progress "$manifest_url" "$manifest" 2 "正在读取百积木安装包清单" ""
  if ! asset_fields "$manifest" "$app_asset"; then
    rm -rf "$work_dir"
    fail_install "百积木缓存中的制品缺失或不完整：$app_asset"
  fi
  set_step 2 "completed" "已找到制品 $app_asset"
  download_with_progress "$mirror_url" "$dmg" 3 "正在下载官方 ChatGPT 桌面应用安装包" "$size_bytes"

  set_step 4 "running" "正在校验应用安装包 SHA256"
  actual="$(shasum -a 256 "$dmg" | awk '{print $1}')"
  if [ "$actual" != "$sha256" ]; then
    rm -rf "$work_dir"
    fail_install "制品 SHA256 不匹配：$app_asset"
  fi

  set_step 4 "running" "正在挂载并安装 ChatGPT 桌面应用"
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
    fail_install "DMG 中未找到受支持的应用包"
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
  set_step 4 "completed" "ChatGPT 桌面应用已安装"
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
    *) fail_install "百积木缓存不包含当前 macOS 架构的 Codex CLI：$arch" ;;
  esac

  work_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-cli.XXXXXX")"
  manifest="$work_dir/latest.json"
  archive="$work_dir/$cli_asset"
  download_with_progress "$manifest_url" "$manifest" 5 "正在读取百积木 CLI 安装包清单" ""
  if ! asset_fields "$manifest" "$cli_asset"; then
    rm -rf "$work_dir"
    fail_install "百积木缓存中的制品缺失或不完整：$cli_asset"
  fi
  download_with_progress "$mirror_url" "$archive" 5 "正在下载官方 Codex CLI 安装包" "$size_bytes"

  set_step 5 "running" "正在校验 Codex CLI 安装包 SHA256"
  actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
  if [ "$actual" != "$sha256" ]; then
    rm -rf "$work_dir"
    fail_install "制品 SHA256 不匹配：$cli_asset"
  fi

  set_step 5 "running" "正在安装 Codex CLI"
  tar -xzf "$archive" -C "$work_dir"
  bin="$(find "$work_dir" -maxdepth 4 -type f \( -name codex -o -name 'codex-*' \) ! -name '*.tar.gz' -perm -111 2>/dev/null | head -n 1)"
  if [ -z "${bin:-}" ]; then
    bin="$(find "$work_dir" -maxdepth 4 -type f \( -name codex -o -name 'codex-*' \) ! -name '*.tar.gz' 2>/dev/null | head -n 1)"
  fi
  if [ -z "${bin:-}" ]; then
    rm -rf "$work_dir"
    fail_install "解压 $cli_asset 后未找到 Codex 可执行文件"
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
  set_step 5 "running" "正在检查 Codex CLI"
  if cli_path="$(resolve_codex_cli)"; then
    cli_install_method="already-installed"
    set_step 5 "completed" "Codex CLI 已可用"
    return
  fi
  install_codex_cli_from_cache
  if ! cli_path="$(resolve_codex_cli)"; then
    fail_install "安装完成后未找到 Codex CLI"
  fi
  set_step 5 "completed" "Codex CLI 已安装"
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
  set_step 6 "running" "正在创建百积木 LLM 凭证并写入 Codex 配置"
  if ! local_api_key="$(create_baijimu_llm_credential)"; then
    error_detail="$(tail -n 5 "$state_dir/baijimu-llm-credential.err" 2>/dev/null | tr '\n' ' ')"
    fail_install "为工作区 $BAIJIMU_WORKSPACE_ID 创建百积木 LLM 凭证失败：$error_detail"
  fi
  if [ -z "$local_api_key" ]; then
    fail_install "百积木 CLI 未返回工作区 $BAIJIMU_WORKSPACE_ID 的 LLM 凭证"
  fi
  shared_cli_token_read=true
  llm_credential_created=true
  write_codex_config "$local_api_key"
  config_written=true
  auth_written=true
  test "$(stat -f '%Lp' "$codex_dir/auth.json")" = "600"
  set_step 6 "completed" "已使用百积木 LLM 凭证写入 Codex 配置"
}

verify_router() {
  set_step 7 "running" "正在验证百积木路由"
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
    fail_install "路由 /responses 健康检查失败：$error_detail"
  fi
  rm -f /tmp/codex-router-responses.json "$router_err"
  if [ "$router_status" != "200" ]; then
    unset local_api_key
    fail_install "路由 /responses 健康检查失败：HTTP $router_status"
  fi
  set_step 7 "completed" "百积木路由验证通过"
}

verify_codex_cli() {
  set_step 8 "running" "正在检查 Codex CLI 版本"
  cli_version="$("$cli_path" --version 2>&1)"
  if smoke_output="$("$cli_path" exec --skip-git-repo-check "Reply exactly OK" 2>&1)"; then
    smoke_exit_code=0
  else
    smoke_exit_code=$?
  fi
  if [ "$smoke_exit_code" -ne 0 ]; then
    smoke_detail="$(awk 'NF { line = $0 } END { print line }' <<< "$smoke_output")"
    fail_install "Codex CLI 冒烟测试失败，退出码 $smoke_exit_code：$smoke_detail"
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
    fail_install "Codex CLI 冒烟测试未返回准确的 OK 响应"
  fi
  cli_smoke=true
  unset smoke_output
  set_step 8 "completed" "Codex CLI 验证通过"
}

write_install_console ""
write_install_console "百积木正在安装 ChatGPT 桌面应用和 Codex"
write_install_console "请保持此窗口打开。"
write_install_console ""
write_status

ensure_codex_app
ensure_codex_cli
configure_codex_terminal
verify_router
unset local_api_key
verify_codex_cli
set_step 9 "completed" "安装配置已完成，桌面启动由 Connector 按档案状态处理"

trap - ERR
complete_pending_steps "skipped" "安装已完成"
write_install_console ""
write_install_console "ChatGPT 桌面应用和 Codex 配置已完成，可以关闭此窗口。"
finish_result true ""
