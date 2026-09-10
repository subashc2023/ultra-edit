from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path


SCRIPT_PATH = Path(__file__).resolve().parents[1] / "release.py"
SPEC = importlib.util.spec_from_file_location("ultra_edit_release", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
release = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = release
SPEC.loader.exec_module(release)


class ReleasePackagingTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self._write_project("1.2.3")

    def _write_project(self, version: str) -> None:
        (self.root / "plugin/claude-code/.claude-plugin").mkdir(parents=True)
        (self.root / "plugin/claude-code/hooks").mkdir(parents=True)
        (self.root / "plugin/claude-code/skills/edit/references").mkdir(parents=True)
        (self.root / "plugin/claude-code/runtime").mkdir(parents=True)
        (self.root / "packaging").mkdir()

        (self.root / "Cargo.toml").write_text(
            f'[package]\nname = "ultra-edit"\nversion = "{version}"\n'
            'license = "MIT OR Apache-2.0"\n',
            encoding="utf-8",
        )
        (self.root / "Cargo.lock").write_text(
            f'version = 4\n\n[[package]]\nname = "ultra-edit"\nversion = "{version}"\n',
            encoding="utf-8",
        )
        manifest = {
            "name": "ultra-edit",
            "version": version,
            "description": "Test plugin",
            "license": "MIT OR Apache-2.0",
            "author": {"name": "Test Author"},
        }
        (self.root / "plugin/claude-code/.claude-plugin/plugin.json").write_text(
            json.dumps(manifest), encoding="utf-8"
        )
        mcp = {
            "mcpServers": {
                "ultra-edit": {
                    "type": "stdio",
                    "command": "${CLAUDE_PLUGIN_ROOT}/runtime/ultra-edit-mcp",
                    "args": ["--root", "${CLAUDE_PROJECT_DIR}"],
                }
            }
        }
        (self.root / "plugin/claude-code/.mcp.json").write_text(
            json.dumps(mcp), encoding="utf-8"
        )
        hooks = {
            "hooks": {
                event: [
                    {
                        "hooks": [
                            {
                                "type": "command",
                                "command": "${CLAUDE_PLUGIN_ROOT}/runtime/ultra-edit-mcp",
                                "args": ["--claude-context", event],
                                "timeout": 10,
                            }
                        ]
                    }
                ]
                for event in ("SessionStart", "SubagentStart")
            }
        }
        (self.root / "plugin/claude-code/hooks/hooks.json").write_text(
            json.dumps(hooks), encoding="utf-8"
        )
        (self.root / "plugin/claude-code/instructions.md").write_text(
            "instructions\r\nsecond line\r\n", encoding="utf-8", newline=""
        )
        (self.root / "plugin/claude-code/skills/edit/SKILL.md").write_text(
            "skill\n", encoding="utf-8"
        )
        (self.root / "plugin/claude-code/skills/edit/references/contract.md").write_text(
            "contract\n", encoding="utf-8"
        )
        (self.root / "plugin/claude-code/runtime/stale.exe").write_bytes(b"stale")
        (self.root / "README.md").write_text("readme\n", encoding="utf-8")
        (self.root / "LICENSE-MIT").write_text("license\n", encoding="utf-8")
        (self.root / "LICENSE-APACHE").write_text("apache license\n", encoding="utf-8")
        (self.root / "THIRD_PARTY_LICENSES.txt").write_bytes(
            b"third-party licenses\r\nsecond line\n"
        )
        (self.root / "RUST_STANDARD_LIBRARY_LICENSES.html").write_bytes(
            b"<html>Rust standard library licenses</html>\r\n"
        )
        (self.root / "MUSL_COPYRIGHT").write_bytes(b"musl copyright\r\n")
        (self.root / "LLVM_LIBUNWIND_LICENSE.txt").write_bytes(
            b"LLVM libunwind license\r\n"
        )
        (self.root / "RUST_COMPILER_BUILTINS_LICENSE.txt").write_bytes(
            b"Rust compiler-builtins license\r\n"
        )
        (self.root / "RUST_LIBM_LICENSE.txt").write_bytes(b"Rust libm license\r\n")
        (self.root / "LLVM_COMPILER_RT_LICENSE.txt").write_bytes(
            b"LLVM compiler-rt license\r\n"
        )
        launcher = (SCRIPT_PATH.parents[1] / "packaging/launcher.sh").read_bytes()
        (self.root / "packaging/launcher.sh").write_bytes(launcher)

    def _binary_dir(self, target_name: str) -> Path:
        target = release.TARGETS[target_name]
        directory = self.root / "build" / target_name
        directory.mkdir(parents=True, exist_ok=True)
        for binary_name in target.binary_names:
            (directory / binary_name).write_bytes(f"{target_name}:{binary_name}".encode())
        return directory

    def _thin_archive(self, target_name: str, directory: Path) -> Path:
        output = directory / f"ultra-edit-plugin-v1.2.3-{target_name}.zip"
        release.package_thin(
            target_name,
            self._binary_dir(target_name),
            output,
            repo_root=self.root,
        )
        return output

    def _all_archives(self) -> tuple[Path, Path]:
        assets = self.root / "assets"
        assets.mkdir(exist_ok=True)
        for target_name in release.TARGETS:
            self._thin_archive(target_name, assets)
        output = assets / "ultra-edit-plugin-v1.2.3-all.zip"
        release.package_all(assets, output, repo_root=self.root)
        return assets, output

    def test_verify_version_checks_tag_cargo_lock_and_plugin(self) -> None:
        self.assertEqual(release.verify_version("v1.2.3", self.root), "1.2.3")
        with self.assertRaisesRegex(release.ReleaseError, "tag version"):
            release.verify_version("v1.2.4", self.root)

        manifest_path = self.root / "plugin/claude-code/.claude-plugin/plugin.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["version"] = "1.2.2"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(release.ReleaseError, "versions do not match"):
            release.verify_version("v1.2.3", self.root)

    def test_verify_version_requires_dual_license_metadata(self) -> None:
        cargo_path = self.root / "Cargo.toml"
        cargo = cargo_path.read_text(encoding="utf-8")
        cargo_path.write_text(cargo.replace("MIT OR Apache-2.0", "MIT"), encoding="utf-8")
        with self.assertRaisesRegex(release.ReleaseError, "package.license"):
            release.verify_version("v1.2.3", self.root)

        cargo_path.write_text(cargo, encoding="utf-8")
        manifest_path = self.root / "plugin/claude-code/.claude-plugin/plugin.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["license"] = "MIT"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(release.ReleaseError, "plugin.json license"):
            release.verify_version("v1.2.3", self.root)

    def test_packaging_rejects_noncanonical_output_name(self) -> None:
        target_name = "x86_64-unknown-linux-musl"
        with self.assertRaisesRegex(release.ReleaseError, "must be named"):
            release.package_thin(
                target_name,
                self._binary_dir(target_name),
                self.root / "latest.zip",
                repo_root=self.root,
            )

    def test_thin_archive_is_complete_clean_deterministic_and_executable(self) -> None:
        target_name = "x86_64-unknown-linux-musl"
        binary_dir = self._binary_dir(target_name)
        archive_name = f"ultra-edit-plugin-v1.2.3-{target_name}.zip"
        first = self.root / "first" / archive_name
        second = self.root / "second" / archive_name
        release.package_thin(target_name, binary_dir, first, repo_root=self.root)
        release.package_thin(target_name, binary_dir, second, repo_root=self.root)
        self.assertEqual(first.read_bytes(), second.read_bytes())
        self.assertEqual(
            release.verify_archive(first, target=target_name, repo_root=self.root), "1.2.3"
        )

        with zipfile.ZipFile(first) as archive:
            infos = archive.infolist()
            names = [info.filename for info in infos]
            self.assertEqual(names, sorted(names))
            self.assertIn(".claude-plugin/plugin.json", names)
            self.assertIn(".mcp.json", names)
            self.assertIn("README.md", names)
            self.assertIn("LICENSE-MIT", names)
            self.assertIn("LICENSE-APACHE", names)
            self.assertIn("THIRD_PARTY_LICENSES.txt", names)
            self.assertIn("RUST_STANDARD_LIBRARY_LICENSES.html", names)
            self.assertIn("MUSL_COPYRIGHT", names)
            self.assertIn("LLVM_LIBUNWIND_LICENSE.txt", names)
            self.assertIn("RUST_COMPILER_BUILTINS_LICENSE.txt", names)
            self.assertIn("RUST_LIBM_LICENSE.txt", names)
            self.assertIn("LLVM_COMPILER_RT_LICENSE.txt", names)
            self.assertEqual(
                archive.read("THIRD_PARTY_LICENSES.txt"),
                (self.root / "THIRD_PARTY_LICENSES.txt").read_bytes(),
            )
            self.assertEqual(
                archive.read("RUST_STANDARD_LIBRARY_LICENSES.html"),
                (self.root / "RUST_STANDARD_LIBRARY_LICENSES.html").read_bytes(),
            )
            self.assertEqual(
                archive.read("MUSL_COPYRIGHT"),
                (self.root / "MUSL_COPYRIGHT").read_bytes(),
            )
            self.assertEqual(
                archive.read("LLVM_LIBUNWIND_LICENSE.txt"),
                (self.root / "LLVM_LIBUNWIND_LICENSE.txt").read_bytes(),
            )
            self.assertEqual(
                archive.read("RUST_COMPILER_BUILTINS_LICENSE.txt"),
                (self.root / "RUST_COMPILER_BUILTINS_LICENSE.txt").read_bytes(),
            )
            self.assertEqual(
                archive.read("RUST_LIBM_LICENSE.txt"),
                (self.root / "RUST_LIBM_LICENSE.txt").read_bytes(),
            )
            self.assertEqual(
                archive.read("LLVM_COMPILER_RT_LICENSE.txt"),
                (self.root / "LLVM_COMPILER_RT_LICENSE.txt").read_bytes(),
            )
            self.assertNotIn("runtime/stale.exe", names)
            self.assertEqual(
                archive.read("instructions.md"), b"instructions\nsecond line\n"
            )
            self.assertEqual(
                {name for name in names if name.startswith("runtime/")},
                {"runtime/ultra-edit", "runtime/ultra-edit-mcp"},
            )
            for info in infos:
                self.assertEqual(info.date_time, release.FIXED_TIMESTAMP)
                mode = stat.S_IMODE(info.external_attr >> 16)
                expected = 0o755 if info.filename.startswith("runtime/") else 0o644
                self.assertEqual(mode, expected)

    def test_verify_archive_rejects_path_traversal(self) -> None:
        archive_path = self.root / "unsafe.zip"
        with zipfile.ZipFile(archive_path, "w") as archive:
            info = zipfile.ZipInfo("../outside", release.FIXED_TIMESTAMP)
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            archive.writestr(info, b"bad")
        with self.assertRaisesRegex(release.ReleaseError, "unsafe archive entry"):
            release.verify_archive(archive_path, repo_root=self.root)

    def test_verify_archive_rejects_paths_that_alias_after_normalization(self) -> None:
        archive_path = self.root / "aliased.zip"
        with zipfile.ZipFile(archive_path, "w") as archive:
            for name in ("aliases//file", "aliases/file"):
                info = zipfile.ZipInfo(name, release.FIXED_TIMESTAMP)
                info.create_system = 3
                info.external_attr = (stat.S_IFREG | 0o644) << 16
                archive.writestr(info, b"collision")
        with self.assertRaisesRegex(release.ReleaseError, "unsafe archive entry"):
            release.verify_archive(archive_path, repo_root=self.root)

    def test_packaging_requires_every_release_document(self) -> None:
        for document in release.REQUIRED_REPO_DOCUMENTS:
            with self.subTest(document=document):
                path = self.root / document
                data = path.read_bytes()
                path.unlink()
                try:
                    with self.assertRaisesRegex(release.ReleaseError, document.replace(".", r"\.")):
                        release.package_thin(
                            "x86_64-unknown-linux-musl",
                            self._binary_dir("x86_64-unknown-linux-musl"),
                            self.root
                            / "ultra-edit-plugin-v1.2.3-x86_64-unknown-linux-musl.zip",
                            repo_root=self.root,
                        )
                finally:
                    path.write_bytes(data)

    def test_verify_archive_rejects_non_executable_runtime(self) -> None:
        target_name = "aarch64-apple-darwin"
        valid = self.root / f"ultra-edit-plugin-v1.2.3-{target_name}.zip"
        release.package_thin(
            target_name, self._binary_dir(target_name), valid, repo_root=self.root
        )
        files, modes = release._read_archive(valid)
        entries = {
            name: (data, 0o644 if name == "runtime/ultra-edit" else modes[name])
            for name, data in files.items()
        }
        invalid = self.root / "invalid.zip"
        release._write_zip(invalid, entries)
        with self.assertRaisesRegex(release.ReleaseError, "expected 0755"):
            release.verify_archive(invalid, target=target_name, repo_root=self.root)

    def test_all_archive_contains_every_target_and_dispatchers(self) -> None:
        _, archive_path = self._all_archives()
        self.assertEqual(
            release.verify_archive(archive_path, all_targets=True, repo_root=self.root),
            "1.2.3",
        )
        with zipfile.ZipFile(archive_path) as archive:
            names = set(archive.namelist())
            runtime = {name for name in names if name.startswith("runtime/")}
            self.assertEqual(runtime, release._all_runtime_names())
            launcher = (self.root / "packaging/launcher.sh").read_bytes()
            self.assertEqual(archive.read("runtime/ultra-edit"), launcher)
            self.assertEqual(archive.read("runtime/ultra-edit-mcp"), launcher)
            self.assertEqual(
                archive.read(
                    "runtime/targets/aarch64-unknown-linux-musl/ultra-edit-mcp"
                ),
                b"aarch64-unknown-linux-musl:ultra-edit-mcp",
            )

    @unittest.skipIf(os.name == "nt", "POSIX launcher execution requires a POSIX host")
    def test_launcher_dispatches_by_platform_and_invoked_basename(self) -> None:
        runtime = self.root / "launcher-test/runtime"
        fake_bin = self.root / "launcher-test/fake-bin"
        runtime.mkdir(parents=True)
        fake_bin.mkdir(parents=True)

        launcher = (self.root / "packaging/launcher.sh").read_bytes()
        for binary_name in ("ultra-edit", "ultra-edit-mcp"):
            launcher_path = runtime / binary_name
            launcher_path.write_bytes(launcher)
            launcher_path.chmod(0o755)
        for target in release.POSIX_TARGETS:
            directory = runtime / "targets" / target.triple
            directory.mkdir(parents=True)
            for binary_name in target.binary_names:
                executable = directory / binary_name
                executable.write_text(
                    f"#!/bin/sh\nprintf '%s\\n' '{target.triple}:{binary_name}' \"$@\"\n",
                    encoding="utf-8",
                )
                executable.chmod(0o755)

        uname = fake_bin / "uname"
        uname.write_text(
            "#!/bin/sh\n"
            "case \"$1\" in\n"
            "  -s) printf '%s\\n' \"$FAKE_SYSTEM\" ;;\n"
            "  -m) printf '%s\\n' \"$FAKE_MACHINE\" ;;\n"
            "  *) exit 64 ;;\n"
            "esac\n",
            encoding="utf-8",
        )
        uname.chmod(0o755)

        platforms = (
            ("Linux", "x86_64", "x86_64-unknown-linux-musl"),
            ("Linux", "aarch64", "aarch64-unknown-linux-musl"),
            ("Darwin", "x86_64", "x86_64-apple-darwin"),
            ("Darwin", "arm64", "aarch64-apple-darwin"),
        )
        for system, machine, target_name in platforms:
            for binary_name in ("ultra-edit", "ultra-edit-mcp"):
                with self.subTest(system=system, machine=machine, binary=binary_name):
                    environment = os.environ.copy()
                    environment.update(
                        {
                            "FAKE_SYSTEM": system,
                            "FAKE_MACHINE": machine,
                            "PATH": f"{fake_bin}{os.pathsep}{environment['PATH']}",
                        }
                    )
                    result = subprocess.run(
                        [runtime / binary_name, "argument with space"],
                        check=True,
                        capture_output=True,
                        env=environment,
                        text=True,
                    )
                    self.assertEqual(
                        result.stdout.splitlines(),
                        [f"{target_name}:{binary_name}", "argument with space"],
                    )

    def test_package_all_requires_all_canonical_thin_archives(self) -> None:
        assets = self.root / "assets"
        assets.mkdir()
        self._thin_archive("x86_64-pc-windows-msvc", assets)
        with self.assertRaisesRegex(release.ReleaseError, "missing canonically named"):
            release.package_all(
                assets,
                assets / "ultra-edit-plugin-v1.2.3-all.zip",
                repo_root=self.root,
            )

    def test_metadata_uses_pinned_archive_and_sorted_complete_checksums(self) -> None:
        assets, archive_path = self._all_archives()
        marketplace_path = assets / "marketplace.json"
        checksums_path = assets / "SHA256SUMS"
        release.write_metadata(
            archive_path,
            "v1.2.3",
            "owner/repository",
            marketplace_path,
            checksums_path,
            assets,
            repo_root=self.root,
        )

        marketplace = json.loads(marketplace_path.read_text(encoding="utf-8"))
        self.assertEqual(marketplace["name"], "ultra-edit")
        self.assertEqual(marketplace["description"], "Test plugin")
        self.assertNotIn("version", marketplace)
        plugin = marketplace["plugins"][0]
        self.assertEqual(plugin["name"], "ultra-edit")
        self.assertNotIn("version", plugin)
        source = plugin["source"]
        self.assertEqual(source["source"], "archive")
        self.assertEqual(
            source["url"],
            "https://github.com/owner/repository/releases/download/"
            "v1.2.3/ultra-edit-plugin-v1.2.3-all.zip",
        )
        self.assertEqual(source["sha256"], hashlib.sha256(archive_path.read_bytes()).hexdigest())

        checksum_lines = checksums_path.read_text(encoding="utf-8").splitlines()
        names = [line.split("  ", 1)[1] for line in checksum_lines]
        self.assertEqual(names, sorted(names))
        self.assertEqual(
            names,
            sorted(path.name for path in assets.iterdir() if path.is_file() and path != checksums_path),
        )
        for line in checksum_lines:
            digest, name = line.split("  ", 1)
            self.assertEqual(digest, hashlib.sha256((assets / name).read_bytes()).hexdigest())

    def _catalog(self, version: str, repository: str = "owner/repository") -> Path:
        assets, archive_path = self._all_archives()
        marketplace_path = assets / "marketplace.json"
        release.write_metadata(
            archive_path,
            f"v{version}",
            repository,
            marketplace_path,
            assets / "SHA256SUMS",
            assets,
            repo_root=self.root,
        )
        catalog, _ = release.update_catalog(
            marketplace_path, f"v{version}", repository, repo_root=self.root
        )
        return catalog

    def test_update_catalog_mirrors_the_generated_release_metadata(self) -> None:
        assets, archive_path = self._all_archives()
        marketplace_path = assets / "marketplace.json"
        release.write_metadata(
            archive_path,
            "v1.2.3",
            "owner/repository",
            marketplace_path,
            assets / "SHA256SUMS",
            assets,
            repo_root=self.root,
        )

        catalog, changed = release.update_catalog(
            marketplace_path, "v1.2.3", "owner/repository", repo_root=self.root
        )
        self.assertTrue(changed)
        self.assertEqual(catalog, self.root / release.CATALOG_PATH)
        self.assertEqual(catalog.read_bytes(), marketplace_path.read_bytes())
        self.assertEqual(release.verify_catalog("owner/repository", self.root), "1.2.3")

        _, changed_again = release.update_catalog(
            marketplace_path, "v1.2.3", "owner/repository", repo_root=self.root
        )
        self.assertFalse(changed_again)

    def test_update_catalog_refuses_a_tag_that_the_metadata_does_not_install(self) -> None:
        catalog = self._catalog("1.2.3")
        before = catalog.read_bytes()
        marketplace_path = self.root / "assets/marketplace.json"

        with self.assertRaisesRegex(release.ReleaseError, "does not match tag"):
            release.update_catalog(
                marketplace_path, "v1.2.4", "owner/repository", repo_root=self.root
            )
        self.assertEqual(catalog.read_bytes(), before)

    def test_update_catalog_refuses_to_move_the_catalog_backwards(self) -> None:
        catalog = self._catalog("1.2.3")
        published = json.loads(catalog.read_text(encoding="utf-8"))
        older = json.loads(json.dumps(published))
        older["plugins"][0]["source"]["url"] = (
            "https://github.com/owner/repository/releases/download/"
            "v1.2.2/ultra-edit-plugin-v1.2.2-all.zip"
        )
        older_path = self.root / "assets/older-marketplace.json"
        older_path.write_text(json.dumps(older, indent=2) + "\n", encoding="utf-8")
        before = catalog.read_bytes()

        with self.assertRaisesRegex(release.ReleaseError, "refusing to move it back"):
            release.update_catalog(
                older_path, "v1.2.2", "owner/repository", repo_root=self.root
            )
        self.assertEqual(catalog.read_bytes(), before)

    def test_verify_catalog_rejects_unpinned_unreleased_and_reformatted_catalogs(self) -> None:
        catalog = self._catalog("1.2.3")
        published = json.loads(catalog.read_text(encoding="utf-8"))

        unpinned = json.loads(json.dumps(published))
        unpinned["plugins"][0]["source"]["sha256"] = "not-a-digest"
        ahead = json.loads(json.dumps(published))
        ahead["plugins"][0]["source"]["url"] = (
            "https://github.com/owner/repository/releases/download/"
            "v9.9.9/ultra-edit-plugin-v9.9.9-all.zip"
        )
        foreign = json.loads(json.dumps(published))
        foreign["plugins"][0]["source"]["url"] = (
            "https://github.com/attacker/repository/releases/download/"
            "v1.2.3/ultra-edit-plugin-v1.2.3-all.zip"
        )
        extra = json.loads(json.dumps(published))
        extra["plugins"].append(published["plugins"][0])

        cases = (
            (unpinned, "SHA-256 digest"),
            (ahead, "unreleased"),
            (foreign, "released all-target archive"),
            (extra, "exactly one plugin"),
        )
        for document, message in cases:
            with self.subTest(message=message):
                catalog.write_text(
                    json.dumps(document, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
                )
                with self.assertRaisesRegex(release.ReleaseError, message):
                    release.verify_catalog("owner/repository", self.root)

        catalog.write_text(json.dumps(published), encoding="utf-8")
        with self.assertRaisesRegex(release.ReleaseError, "generated catalog formatting"):
            release.verify_catalog("owner/repository", self.root)

    def test_repository_catalog_matches_the_committed_project(self) -> None:
        self.assertRegex(release.verify_catalog(), rf"^{release.SEMVER_PATTERN}$")

    def test_metadata_rejects_every_output_path_collision_before_writing(self) -> None:
        collisions = ("archive-marketplace", "archive-checksums", "marketplace-checksums")
        for collision in collisions:
            with self.subTest(collision=collision):
                assets, archive_path = self._all_archives()
                marketplace_path = assets / "marketplace.json"
                checksums_path = assets / "SHA256SUMS"
                if collision == "archive-marketplace":
                    marketplace_path = archive_path
                elif collision == "archive-checksums":
                    checksums_path = archive_path
                else:
                    checksums_path = marketplace_path
                archive_before = archive_path.read_bytes()

                with self.assertRaisesRegex(release.ReleaseError, "must be distinct"):
                    release.write_metadata(
                        archive_path,
                        "v1.2.3",
                        "owner/repository",
                        marketplace_path,
                        checksums_path,
                        assets,
                        repo_root=self.root,
                    )

                self.assertEqual(archive_path.read_bytes(), archive_before)
                if marketplace_path != archive_path:
                    self.assertFalse(marketplace_path.exists())
                if checksums_path not in (archive_path, marketplace_path):
                    self.assertFalse(checksums_path.exists())


if __name__ == "__main__":
    unittest.main()
