#!/bin/sh
set -eu
repo_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
skill_dir="$repo_dir/skills/lexmount-browser"
dist_dir="$repo_dir/dist"
staging_dir="$(mktemp -d)"
trap 'rm -rf "$staging_dir"' EXIT INT TERM
mkdir -p "$dist_dir"
rm -f "$dist_dir/lexmount-browser.zip"

for required_binary in \
  "$skill_dir/bin/browser-cli" \
  "$skill_dir/bin/browser-cli.exe"
do
  if [ ! -f "$required_binary" ]; then
    echo "Missing required Skill binary: $required_binary" >&2
    exit 1
  fi
done

file "$skill_dir/bin/browser-cli" | grep -q 'Mach-O 64-bit.*arm64' || {
  echo "bin/browser-cli is not a macOS arm64 executable" >&2
  exit 1
}
file "$skill_dir/bin/browser-cli.exe" | grep -q 'PE32+ executable.*x86-64' || {
  echo "bin/browser-cli.exe is not a Windows x64 executable" >&2
  exit 1
}

(
  cd "$skill_dir"
  find . -type f ! -name '.DS_Store' \
    \( ! -path './bin/*' -o -path './bin/browser-cli' -o -path './bin/browser-cli.exe' \) \
    -print | LC_ALL=C sort |
    while IFS= read -r relative_path; do
      mkdir -p "$staging_dir/$(dirname -- "$relative_path")"
      cp "$relative_path" "$staging_dir/$relative_path"
    done
)

find "$staging_dir" -type d -exec chmod 0755 {} +
find "$staging_dir" -type f -exec chmod 0644 {} +
find "$staging_dir/scripts" -type f \( -name '*.sh' -o -name '*.ps1' \) -exec chmod 0755 {} +
find "$staging_dir/bin" -type f -exec chmod 0755 {} +
find "$staging_dir" -exec touch -t 198001010000 {} +

(
  cd "$staging_dir"
  find . -type f -print | LC_ALL=C sort | sed 's|^\./||' |
    zip -X -9 -q "$dist_dir/lexmount-browser.zip" -@
)
echo "$dist_dir/lexmount-browser.zip"
