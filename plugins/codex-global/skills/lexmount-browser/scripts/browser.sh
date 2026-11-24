#!/bin/sh
set -eu
skill_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
binary="$skill_dir/bin/browser-cli"
auth="$skill_dir/scripts/device_auth.py"
api='https://api.lexmount.com'
if [ -n "${LEXMOUNT_BASE_URL:-}" ] && [ "${LEXMOUNT_BASE_URL%/}" != "$api" ]; then
  echo 'LEXMOUNT_BASE_URL conflicts with the Global cloud edition.' >&2
  exit 2
fi
if [ -n "${LEXMOUNT_API_KEY:-}${LEXMOUNT_PROJECT_ID:-}" ]; then
  echo 'Unset API-key environment credentials; this edition uses its own device-authorized profile.' >&2
  exit 2
fi
help_requested=false
for argument in "$@"; do
  case "$argument" in
    --help|-h) help_requested=true ;;
    --connect-base-url|--connect-base-url=*|--credentials-file|--credentials-file=*|--browser|--no-open)
      echo 'The Global cloud edition fixes its site, credential profile and device authorization method.' >&2
      exit 2 ;;
  esac
done
export LEXMOUNT_BASE_URL="$api"
export LEXMOUNT_BROWSER_CREDENTIALS_FILE="${LEXMOUNT_BROWSER_CREDENTIALS_FILE:-${HOME:?HOME is required}/.config/lexmount/browser-cli/sites/global-cloud/credentials.json}"
case "$LEXMOUNT_BROWSER_CREDENTIALS_FILE" in /*) ;; *) echo 'Use an absolute credential-file path.' >&2; exit 2 ;; esac
if [ "${1:-}" = auth ]; then
  if [ "$help_requested" = true ]; then exec python3 "$auth" --help; fi
  operation="${2:-status}"
  shift
  if [ "$#" -gt 0 ]; then shift; fi
  case "$operation" in
    login|start) exec python3 "$auth" start "$@" ;;
    poll|status|cancel|logout) exec python3 "$auth" "$operation" "$@" ;;
    *) echo 'Use auth login, poll, status, cancel or logout.' >&2; exit 2 ;;
  esac
fi
if [ ! -x "$binary" ]; then
  echo 'Skill-local CLI is missing. Run scripts/bootstrap.sh.' >&2
  exit 127
fi
if [ "$("$binary" --version)" != 'browser-cli 1.5.0' ]; then
  echo 'Expected the pinned CLI 1.5.0. Run scripts/bootstrap.sh.' >&2
  exit 2
fi
if [ "$help_requested" = false ]; then
case "${1:-}" in
  version|--version|--help|-h) ;;
  auth) ;;
  *) python3 "$auth" check >/dev/null ;;
esac
fi
exec "$binary" "$@"
