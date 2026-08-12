#!/bin/sh
set -eu
repo_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
skill_dir="$repo_dir/skills/lexmount-browser"
dist_dir="$repo_dir/dist"
staging_dir="$(mktemp -d)"
trap 'rm -rf "$staging_dir"' EXIT INT TERM
mkdir -p "$dist_dir"
rm -f "$dist_dir/lexmount-browser.zip"

(
  cd "$skill_dir"
  find . -type f ! -path './bin/*' ! -name '.DS_Store' -print | LC_ALL=C sort |
    while IFS= read -r relative_path; do
      mkdir -p "$staging_dir/$(dirname -- "$relative_path")"
      cp "$relative_path" "$staging_dir/$relative_path"
    done
)

find "$staging_dir" -type d -exec chmod 0755 {} +
find "$staging_dir" -type f -exec chmod 0644 {} +
find "$staging_dir/scripts" -type f \( -name '*.sh' -o -name '*.ps1' \) -exec chmod 0755 {} +
find "$staging_dir" -exec touch -t 198001010000 {} +

(
  cd "$staging_dir"
  find . -type f -print | LC_ALL=C sort | sed 's|^\./||' |
    zip -X -9 -q "$dist_dir/lexmount-browser.zip" -@
)
echo "$dist_dir/lexmount-browser.zip"
