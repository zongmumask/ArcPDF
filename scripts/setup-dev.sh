#!/bin/sh
# Check local development prerequisites. Does not install system packages.
set -e
cd "$(dirname "$0")/.."

fail=0
for tool in git python3 rustc cargo; do
  if command -v "$tool" >/dev/null 2>&1; then
    printf 'OK   %s: %s\n' "$tool" "$(command -v "$tool")"
  else
    printf 'MISS %s\n' "$tool"
    fail=1
  fi
done

# gn/ninja are needed for real PDFium builds (Phase 1-B), not for Phase 1-A dry runs.
for tool in gn ninja; do
  if command -v "$tool" >/dev/null 2>&1; then
    printf 'OK   %s: %s\n' "$tool" "$(command -v "$tool")"
  else
    printf 'INFO %s not found (needed for Phase 1-B real PDFium builds)\n' "$tool"
  fi
done

exit "$fail"
