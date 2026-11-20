#!/usr/bin/env python3
"""Offline checks for shared-source packaging, not client acceptance."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("packager", ROOT / "scripts/package-plugins.py")
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)


class PluginPackages(unittest.TestCase):
    def test_candidates(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            for profile, (_, manifest_dir, skill) in packager.PROFILES.items():
                with self.subTest(profile=profile):
                    archive = packager.build(profile, output)
                    original = archive.read_bytes()
                    self.assertEqual(original, packager.build(profile, output).read_bytes())
                    self.assertEqual(archive.with_suffix(".zip.sha256").read_text().split()[0],
                                     hashlib.sha256(original).hexdigest())
                    with zipfile.ZipFile(archive) as bundle:
                        self.assertIsNone(bundle.testzip())
                        prefix = "lexmount-cloud-browser/"
                        names = bundle.namelist()
                        self.assertEqual(len(names), len(set(names)))
                        for name in names:
                            self.assertTrue(name.startswith(prefix))
                            self.assertNotIn("..", Path(name).parts)
                            self.assertNotIn("bin", Path(name).parts)
                        for path, data in packager.source_files(ROOT / "skills" / skill):
                            self.assertEqual(bundle.read(f"{prefix}skills/{skill}/{path}"), data)
                        manifest = json.loads(bundle.read(f"{prefix}{manifest_dir}/plugin.json"))
                        self.assertEqual(manifest["name"], "lexmount-cloud-browser")
                        self.assertEqual(bundle.read(prefix + "LICENSE"), (ROOT / "LICENSE").read_bytes())
                        self.assertEqual(prefix + ".mcp.json" in names, profile == "codex-mcp")
                        if profile.startswith("codex"):
                            for key in ("logo", "composerIcon"):
                                self.assertIn(prefix + manifest["interface"][key].removeprefix("./"), names)
                        if profile == "codex-mcp":
                            self.assertFalse(any("/scripts/" in name for name in names))
                            config = json.loads(bundle.read(prefix + ".mcp.json"))
                            server = config["mcpServers"]["lexmount-cloud-browser"]
                            self.assertEqual(server["url"], "https://browser.lexmount.cn/chatgpt-app/mcp")
                            self.assertEqual(server["oauth_resource"], server["url"])
                            self.assertEqual(set(server), {"type", "url", "oauth_resource"})

    def test_symlink_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "real").write_text("fixture")
            try:
                (root / "link").symlink_to(root / "real")
            except OSError:
                self.skipTest("Host does not permit creating symlinks")
            with self.assertRaises(ValueError):
                list(packager.source_files(root))


if __name__ == "__main__":
    unittest.main()
