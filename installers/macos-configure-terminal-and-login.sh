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
  local work_dir bin codex_version machine target codex_home package_root releases_dir
  local release_dir release_temp current_link install_target install_temp lock_dir lock_attempt
  local shell_name profile line
  verify_sha256 "$archive" "$expected_sha256"

  work_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-cli.XXXXXX")"
  lock_dir=""
  cleanup_cli() {
    rm -rf "$work_dir"
    if [ -n "$lock_dir" ]; then
      rmdir "$lock_dir" 2>/dev/null || true
    fi
  }
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

  codex_version="$("$bin" --version | awk '{print $NF}')"
  case "$codex_version" in
    [0-9]*.[0-9]*.[0-9]*) ;;
    *)
      echo "Codex CLI 返回了无效版本：$codex_version" >&2
      return 1
      ;;
  esac
  machine="$(uname -m)"
  case "$machine" in
    arm64) target="aarch64-apple-darwin" ;;
    x86_64) target="x86_64-apple-darwin" ;;
    *)
      echo "不支持的 macOS 处理器架构：$machine" >&2
      return 1
      ;;
  esac

  codex_home="$HOME/.codex"
  package_root="$codex_home/packages/standalone"
  releases_dir="$package_root/releases"
  release_dir="$releases_dir/$codex_version-$target-${expected_sha256%${expected_sha256#????????????}}"
  release_temp="$releases_dir/.install-$codex_version-$target-$$"
  current_link="$package_root/current"
  mkdir -p "$releases_dir" "$HOME/.local/bin"
  chmod 700 "$codex_home" "$codex_home/packages" "$package_root" "$releases_dir" 2>/dev/null || true

  lock_dir="$package_root/install.lock.d"
  lock_attempt=0
  while ! mkdir -m 700 "$lock_dir" 2>/dev/null; do
    lock_attempt=$((lock_attempt + 1))
    if [ "$lock_attempt" -ge 300 ]; then
      echo "等待 Codex CLI 安装锁超时：$lock_dir" >&2
      return 1
    fi
    sleep 0.1
  done

  rm -rf "$release_temp"
  mkdir -m 700 "$release_temp"
  install -m 755 "$bin" "$release_temp/codex"
  "$release_temp/codex" --version >/dev/null
  "$release_temp/codex" app-server --help >/dev/null
  "$release_temp/codex" app-server proxy --help >/dev/null
  "$release_temp/codex" app-server daemon --help >/dev/null
  if [ -e "$release_dir/codex" ]; then
    "$release_dir/codex" --version >/dev/null
    rm -rf "$release_temp"
  else
    mv "$release_temp" "$release_dir"
  fi

  ln -sfn "$release_dir" "$current_link"

  install_target="$HOME/.local/bin/codex"
  install_temp="$HOME/.local/bin/.codex.install.$$"
  rm -f "$install_temp"
  ln -s "$current_link/codex" "$install_temp"
  "$install_temp" --version >/dev/null
  "$install_temp" app-server --help >/dev/null
  "$install_temp" app-server proxy --help >/dev/null
  "$install_temp" app-server daemon --help >/dev/null
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
  rmdir "$lock_dir"
  lock_dir=""
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
