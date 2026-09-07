#!/usr/bin/env bash
set -Eeuo pipefail

installer="${1:?installer path is required}"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/codex-package-test.XXXXXX")"
cleanup() { rm -rf "$test_root"; }
trap cleanup EXIT

package_root="$test_root/package"
mkdir -p "$package_root/bin" "$package_root/codex-path" "$package_root/codex-resources"
case "$(uname -m)" in
  arm64) target="aarch64-apple-darwin" ;;
  x86_64) target="x86_64-apple-darwin" ;;
  *) exit 2 ;;
esac
cat > "$package_root/codex-package.json" <<EOF
{"layoutVersion":1,"version":"9.8.7","target":"$target","variant":"codex","entrypoint":"bin/codex","resourcesDir":"codex-resources","pathDir":"codex-path"}
EOF
cat > "$package_root/bin/codex" <<'EOF'
#!/bin/sh
if [ "$1" = "--version" ]; then echo "codex-cli 9.8.7"; exit 0; fi
if [ "$1" = "app-server" ]; then exit 0; fi
exit 2
EOF
cat > "$package_root/bin/codex-code-mode-host" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$package_root/codex-path/rg" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod 755 "$package_root/bin/codex" "$package_root/bin/codex-code-mode-host" "$package_root/codex-path/rg"

archive="$test_root/codex-package.tar.gz"
tar -czf "$archive" -C "$package_root" .
checksum="$(shasum -a 256 "$archive" | awk '{print $1}')"
HOME="$test_root/home" SHELL=/bin/zsh bash "$installer" install-cli "$archive" "$checksum" >/dev/null

test "$(HOME="$test_root/home" "$test_root/home/.local/bin/codex" --version)" = "codex-cli 9.8.7"
test -x "$test_root/home/.local/bin/codex-code-mode-host"
test -x "$test_root/home/.codex/packages/standalone/current/codex-path/rg"
test "$(readlink "$test_root/home/.local/bin/codex")" = "$test_root/home/.codex/packages/standalone/current/bin/codex"
test "$(readlink "$test_root/home/.local/bin/codex-code-mode-host")" = "$test_root/home/.codex/packages/standalone/current/bin/codex-code-mode-host"
