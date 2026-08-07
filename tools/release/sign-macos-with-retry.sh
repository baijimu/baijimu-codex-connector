#!/usr/bin/env bash

set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "Usage: $0 <binary> <signing-identity>" >&2
  exit 2
fi

binary="$1"
signing_identity="$2"
codesign_bin="${CODESIGN_BIN:-codesign}"
sleep_bin="${SLEEP_BIN:-sleep}"
max_attempts="${CODESIGN_MAX_ATTEMPTS:-4}"
retry_delay_seconds="${CODESIGN_RETRY_DELAY_SECONDS:-5}"

if [ ! -f "$binary" ]; then
  echo "macOS signing target does not exist: $binary" >&2
  exit 2
fi

case "$max_attempts" in
  ''|*[!0-9]*|0)
    echo "CODESIGN_MAX_ATTEMPTS must be a positive integer" >&2
    exit 2
    ;;
esac

case "$retry_delay_seconds" in
  ''|*[!0-9]*)
    echo "CODESIGN_RETRY_DELAY_SECONDS must be a non-negative integer" >&2
    exit 2
    ;;
esac

attempt=1
while [ "$attempt" -le "$max_attempts" ]; do
  echo "Signing $binary with a trusted timestamp (attempt $attempt/$max_attempts)"

  sign_status=0
  sign_output="$("$codesign_bin" \
    --force \
    --options runtime \
    --timestamp \
    --sign "$signing_identity" \
    "$binary" 2>&1)" || sign_status=$?
  if [ -n "$sign_output" ]; then
    printf '%s\n' "$sign_output"
  fi

  retry_reason=""
  if [ "$sign_status" -eq 0 ]; then
    metadata_status=0
    metadata="$("$codesign_bin" --display --verbose=4 "$binary" 2>&1)" || metadata_status=$?
    if [ -n "$metadata" ]; then
      printf '%s\n' "$metadata"
    fi

    if [ "$metadata_status" -ne 0 ]; then
      echo "Unable to inspect the macOS signature after signing" >&2
      exit "$metadata_status"
    fi

    if printf '%s\n' "$metadata" | grep -Eiq '^[[:space:]]*Timestamp=[[:space:]]*(none)?[[:space:]]*$'; then
      retry_reason="codesign completed without a trusted timestamp"
    elif ! printf '%s\n' "$metadata" | grep -Eq '^[[:space:]]*Timestamp=.+$'; then
      retry_reason="codesign metadata does not contain a trusted timestamp"
    else
      "$codesign_bin" --verify --strict --verbose=2 "$binary"
      echo "macOS signature and trusted timestamp verified: $binary"
      exit 0
    fi
  elif printf '%s\n' "$sign_output" | grep -Eiq 'timestamp'; then
    retry_reason="Apple timestamp service rejected or omitted the timestamp"
  else
    echo "codesign failed with a non-timestamp error; not retrying" >&2
    exit "$sign_status"
  fi

  echo "$retry_reason" >&2
  if [ "$attempt" -eq "$max_attempts" ]; then
    echo "macOS signing failed after $max_attempts timestamp attempts" >&2
    exit 1
  fi

  echo "Retrying macOS timestamp signing in ${retry_delay_seconds}s" >&2
  "$sleep_bin" "$retry_delay_seconds"
  attempt=$((attempt + 1))
  if [ "$retry_delay_seconds" -lt 30 ]; then
    retry_delay_seconds=$((retry_delay_seconds * 2))
    if [ "$retry_delay_seconds" -gt 30 ]; then
      retry_delay_seconds=30
    fi
  fi
done
