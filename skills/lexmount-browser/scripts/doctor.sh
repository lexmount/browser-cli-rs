#!/bin/sh
set -eu
skill_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
if [ -x "$skill_dir/bin/browser-cli" ]; then exec "$skill_dir/bin/browser-cli" doctor; fi
command -v browser-cli >/dev/null 2>&1 || { echo '{"ok":false,"error":"command_not_found","message":"Run bootstrap.sh first."}'; exit 1; }
exec browser-cli doctor
