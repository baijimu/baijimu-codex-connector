#!/usr/bin/env bash
set -Eeuo pipefail
export PATH="/usr/bin:/bin:/usr/sbin:/sbin"

verify_sha256() {
  local archive="$1"
  local expected="$2"
  local actual
  actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
  if [ "$actual" != "$expected" ]; then
    echo "安装制品 SHA256 不匹配：$(basename "$archive")" >&2
    return 1
  fi
}

install_app() {
  local dmg="$1"
  local expected_sha256="$2"
  local mount_dir source_app app_path
  verify_sha256 "$dmg" "$expected_sha256"

  mount_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-dmg.XXXXXX")"
  cleanup_mount() {
    if mount | grep -Fq " on $mount_dir "; then
      hdiutil detach "$mount_dir" -quiet || true
    fi
    rm -rf "$mount_dir"
  }
  trap cleanup_mount EXIT

  hdiutil attach "$dmg" -mountpoint "$mount_dir" -nobrowse -quiet
  if [ -d "$mount_dir/ChatGPT.app" ]; then
    source_app="$mount_dir/ChatGPT.app"
  elif [ -d "$mount_dir/Codex.app" ]; then
    source_app="$mount_dir/Codex.app"
  else
    source_app="$(find "$mount_dir" -maxdepth 1 -name '*.app' -type d | head -n 1)"
  fi
  if [ -z "${source_app:-}" ] || [ ! -d "$source_app" ]; then
    echo "DMG 中未找到受支持的 ChatGPT 桌面应用包" >&2
    return 1
  fi

  app_path="/Applications/$(basename "$source_app")"
  ditto "$source_app" "$app_path"
  hdiutil detach "$mount_dir" -quiet
  rm -rf "$mount_dir"
  trap - EXIT
  xattr -dr com.apple.quarantine "$app_path" 2>/dev/null || true
  test -d "$app_path"
  printf '%s\n' "$app_path"
}

install_cli() {
  local archive="$1"
  local expected_sha256="$2"
  local work_dir bin install_target install_temp shell_name profile line
  verify_sha256 "$archive" "$expected_sha256"

  work_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-cli.XXXXXX")"
  cleanup_cli() { rm -rf "$work_dir"; }
  trap cleanup_cli EXIT
  tar -xzf "$archive" -C "$work_dir"
  bin="$(find "$work_dir" -maxdepth 4 -type f \( -name codex -o -name 'codex-*' \) ! -name '*.tar.gz' -perm -111 2>/dev/null | head -n 1)"
  if [ -z "${bin:-}" ]; then
    bin="$(find "$work_dir" -maxdepth 4 -type f \( -name codex -o -name 'codex-*' \) ! -name '*.tar.gz' 2>/dev/null | head -n 1)"
  fi
  if [ -z "${bin:-}" ]; then
    echo "解压 Codex CLI 安装包后未找到可执行文件" >&2
    return 1
  fi

  mkdir -p "$HOME/.local/bin"
  install_target="$HOME/.local/bin/codex"
  install_temp="$HOME/.local/bin/.codex.install.$$"
  rm -f "$install_temp"
  install -m 755 "$bin" "$install_temp"
  "$install_temp" --version >/dev/null
  "$install_temp" app-server --help >/dev/null
  "$install_temp" app-server proxy --help >/dev/null
  mv -f "$install_temp" "$install_target"
  xattr -d com.apple.quarantine "$install_target" 2>/dev/null || true

  shell_name="${SHELL##*/}"
  case "$shell_name" in
    zsh) profile="$HOME/.zprofile" ;;
    bash) profile="$HOME/.bash_profile" ;;
    sh|dash|ksh) profile="$HOME/.profile" ;;
    *)
      echo "无法确定当前用户登录 Shell 的 PATH 配置文件：${SHELL:-<empty>}" >&2
      return 1
      ;;
  esac
  line='export PATH="$HOME/.local/bin:$PATH"'
  if [ ! -f "$profile" ] || ! grep -Fq '.local/bin' "$profile"; then
    printf '\n# 由百积木 Codex 安装器添加\n%s\n' "$line" >> "$profile"
  fi
  rm -rf "$work_dir"
  trap - EXIT
  printf '%s\n' "$install_target"
}

action="${1:-}"
case "$action" in
  install-app)
    [ "$#" -eq 3 ] || { echo "install-app 参数无效" >&2; exit 2; }
    install_app "$2" "$3"
    ;;
  install-cli)
    [ "$#" -eq 3 ] || { echo "install-cli 参数无效" >&2; exit 2; }
    install_cli "$2" "$3"
    ;;
  *)
    echo "不支持的 macOS 原生安装动作：${action:-<empty>}" >&2
    exit 2
    ;;
esac
