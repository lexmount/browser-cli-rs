#!/bin/sh
set -eu
repo_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
source_dir="$repo_dir/plugins/codex-global"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
mkdir -p "$repo_dir/dist"
output="$repo_dir/dist/lexmount-browser-global-4.0.0.zip"
for relative in plugin.json assets/icon.svg \
  skills/lexmount-browser/SKILL.md \
  skills/lexmount-browser/agents/openai.yaml \
  skills/lexmount-browser/references/authentication.md \
  skills/lexmount-browser/references/commands.md \
  skills/lexmount-browser/scripts/bootstrap.sh \
  skills/lexmount-browser/scripts/browser.sh \
  skills/lexmount-browser/scripts/device_auth.py; do
  mkdir -p "$temporary/$(dirname -- "$relative")"
  cp "$source_dir/$relative" "$temporary/$relative"
done
find "$temporary" -type d -exec chmod 0755 {} +
find "$temporary" -type f -exec chmod 0644 {} +
chmod 0755 "$temporary"/skills/lexmount-browser/scripts/*
find "$temporary" -exec touch -t 198001010000 {} +
rm -f "$output"
(cd "$temporary" && find . -type f -print | LC_ALL=C sort | sed 's|^\./||' | zip -X -9 -q "$output" -@)
echo "$output"
