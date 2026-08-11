#!/bin/sh
set -eu
repo_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
skill_dir="$repo_dir/skills/lexmount-browser"
dist_dir="$repo_dir/dist"
mkdir -p "$dist_dir"
rm -f "$dist_dir/lexmount-browser.zip"
python3 - "$repo_dir/skills" "$dist_dir/lexmount-browser.zip" <<'PY'
import pathlib
import sys
import zipfile

root = pathlib.Path(sys.argv[1])
output = pathlib.Path(sys.argv[2])
files = sorted(path for path in (root / "lexmount-browser").rglob("*") if path.is_file())
with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
    for path in files:
        info = zipfile.ZipInfo(path.relative_to(root).as_posix(), (1980, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = (0o755 if path.suffix in {".sh", ".ps1"} else 0o644) << 16
        archive.writestr(info, path.read_bytes())
PY
echo "$dist_dir/lexmount-browser.zip"
