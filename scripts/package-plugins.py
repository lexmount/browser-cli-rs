#!/usr/bin/env python3
"""Build client candidates from shared Skills; Python is build-time only."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import zipfile

ROOT = Path(__file__).resolve().parents[1]
PROFILES = {
    "codex-cli": ("codex/cli", ".codex-plugin", "lexmount-browser"),
    "codex-mcp": ("codex/mcp", ".codex-plugin", "lexmount-browser-mcp"),
    "claude-cli": ("claude", ".claude-plugin", "lexmount-browser"),
}


def source_files(directory):
    for path in sorted(directory.rglob("*")):
        relative = path.relative_to(directory)
        if any(part in {"bin", ".DS_Store", "__pycache__"} for part in relative.parts):
            continue
        if path.is_symlink():
            raise ValueError(f"Symlink cannot be packaged: {path}")
        if path.is_file():
            yield relative.as_posix(), path.read_bytes()


def build(profile, output):
    template, manifest_dir, skill = PROFILES[profile]
    name = "lexmount-cloud-browser"
    files = dict(source_files(ROOT / "plugins" / template / name))
    manifest = json.loads(files[f"{manifest_dir}/plugin.json"])
    version = manifest["version"]
    if manifest["name"] != name or not re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z.+-]*", version):
        raise ValueError("Invalid plugin name or version")
    files.update((f"skills/{skill}/{path}", data)
                 for path, data in source_files(ROOT / "skills" / skill))
    files["LICENSE"] = (ROOT / "LICENSE").read_bytes()
    if manifest_dir == ".codex-plugin":
        files["assets/lexmount-icon.png"] = (ROOT / "plugins/assets/lexmount-icon.png").read_bytes()
    if profile == "codex-mcp":
        if manifest.get("mcpServers") != "./.mcp.json" or ".mcp.json" not in files:
            raise ValueError("Remote MCP manifest must reference its bundled config")
    elif "mcpServers" in manifest or ".mcp.json" in files:
        raise ValueError("CLI profile must not acquire an MCP connection")

    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"{name}-{profile}-{version}.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
        for path, data in sorted(files.items()):
            entry = zipfile.ZipInfo(f"{name}/{path}", (1980, 1, 1, 0, 0, 0))
            entry.create_system = 3
            entry.external_attr = (0o100755 if path.endswith((".sh", ".ps1")) else 0o100644) << 16
            entry.compress_type = zipfile.ZIP_DEFLATED
            bundle.writestr(entry, data)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(".zip.sha256").write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
    return archive


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", choices=PROFILES)
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    print(build(args.profile, args.output))
