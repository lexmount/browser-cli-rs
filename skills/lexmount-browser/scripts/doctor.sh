#!/bin/sh
set -eu
skill_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
if [ -x "$skill_dir/bin/browser-cli" ]; then exec "$skill_dir/bin/browser-cli" doctor; fi
echo '{"ok":false,"error":"command_not_found","message":"Skill-local browser-cli is missing. Run scripts/bootstrap.sh first."}'
exit 1
