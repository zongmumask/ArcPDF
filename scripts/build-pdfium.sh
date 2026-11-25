#!/bin/sh
# Thin wrapper for users who prefer a .sh entry point.
# Main implementation stays in Python: manifest/JSON/SHA/target logic.
set -e
exec python3 "$(dirname "$0")/build-pdfium" "$@"
