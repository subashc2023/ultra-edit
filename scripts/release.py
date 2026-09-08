#!/usr/bin/env python3
"""Build and validate deterministic ultra-edit release archives."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
import tempfile
import tomllib
import zipfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Mapping, Sequence
from urllib.parse import quote


REPO_ROOT = Path(__file__).resolve().parents[1]
PLUGIN_PATH = Path("plugin/claude-code")
LAUNCHER_PATH = Path("packaging/launcher.sh")
FIXED_TIMESTAMP = (1980, 1, 1, 0, 0, 0)
MAX_ARCHIVE_SIZE = 256 * 1024 * 1024
MAX_UNCOMPRESSED_SIZE = 512 * 1024 * 1024
MAX_ARCHIVE_ENTRIES = 10_000
DUAL_LICENSE = "MIT OR Apache-2.0"
EXACT_NOTICE_DOCUMENTS = (
    "THIRD_PARTY_LICENSES.txt",
    "RUST_STANDARD_LIBRARY_LICENSES.html",
    "MUSL_COPYRIGHT",
    "LLVM_LIBUNWIND_LICENSE.txt",
    "RUST_COMPILER_BUILTINS_LICENSE.txt",
    "RUST_LIBM_LICENSE.txt",
    "LLVM_COMPILER_RT_LICENSE.txt",
)
EXACT_NOTICE_NAMES = frozenset(name.casefold() for name in EXACT_NOTICE_DOCUMENTS)
REQUIRED_REPO_DOCUMENTS = (
    "README.md",
    "LICENSE-MIT",
    "LICENSE-APACHE",
    *EXACT_NOTICE_DOCUMENTS,
)
SEMVER_PATTERN = r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)"
TAG_RE = re.compile(rf"^v(?P<version>{SEMVER_PATTERN})$")
REPOSITORY_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
TEXT_SUFFIXES = frozenset({".json", ".md", ".txt", ".toml", ".yaml", ".yml", ".sh"})


class ReleaseError(RuntimeError):
    """A release input or artifact is invalid."""


@dataclass(frozen=True)
class Target:
    triple: str
    windows: bool = False

    @property
    def binary_names(self) -> tuple[str, str]:
        suffix = ".exe" if self.windows else ""
        return (f"ultra-edit{suffix}", f"ultra-edit-mcp{suffix}")


TARGETS = {
    target.triple: target
    for target in (
        Target("x86_64-pc-windows-msvc", windows=True),
        Target("x86_64-unknown-linux-musl"),
        Target("aarch64-unknown-linux-musl"),
        Target("x86_64-apple-darwin"),
        Target("aarch64-apple-darwin"),
    )
}
POSIX_TARGETS = tuple(target for target in TARGETS.values() if not target.windows)
WINDOWS_TARGET = TARGETS["x86_64-pc-windows-msvc"]
THIN_ARCHIVE_RE = re.compile(
    rf"^ultra-edit-plugin-v(?P<version>{SEMVER_PATTERN})-"
    rf"(?P<target>{'|'.join(re.escape(target) for target in TARGETS)})\.zip$"
)


def _root(repo_root: Path | None) -> Path:
    return REPO_ROOT if repo_root is None else Path(repo_root)


def _read_json_bytes(data: bytes, source: str) -> dict[str, object]:
    try:
        value = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ReleaseError(f"{source} is not valid UTF-8 JSON: {error}") from error
    if not isinstance(value, dict):
        raise ReleaseError(f"{source} must contain a JSON object")
    return value


def _read_json_file(path: Path) -> dict[str, object]:
    try:
        return _read_json_bytes(path.read_bytes(), str(path))
    except OSError as error:
        raise ReleaseError(f"cannot read {path}: {error}") from error


def _read_toml(path: Path) -> dict[str, object]:
    try:
        value = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ReleaseError(f"cannot read TOML from {path}: {error}") from error
    if not isinstance(value, dict):
        raise ReleaseError(f"{path} must contain a TOML table")
    return value


def _require_semver(value: object, source: str) -> str:
    if not isinstance(value, str) or re.fullmatch(SEMVER_PATTERN, value) is None:
        raise ReleaseError(f"{source} must be a MAJOR.MINOR.PATCH version")
    return value


def version_from_tag(tag: str) -> str:
    match = TAG_RE.fullmatch(tag)
    if match is None:
        raise ReleaseError(f"tag {tag!r} must have the form vMAJOR.MINOR.PATCH")
    return match.group("version")


def project_version(repo_root: Path | None = None) -> str:
    root = _root(repo_root)
    cargo_manifest = _read_toml(root / "Cargo.toml")
    package = cargo_manifest.get("package")
    if not isinstance(package, dict) or package.get("name") != "ultra-edit":
        raise ReleaseError("Cargo.toml must define package.name as 'ultra-edit'")
    if package.get("license") != DUAL_LICENSE:
        raise ReleaseError(f"Cargo.toml package.license must be {DUAL_LICENSE!r}")
    cargo_version = _require_semver(package.get("version"), "Cargo.toml package.version")

    plugin_manifest = _read_json_file(root / PLUGIN_PATH / ".claude-plugin/plugin.json")
    plugin_version = _validate_plugin_manifest(plugin_manifest, "plugin.json")

    cargo_lock = _read_toml(root / "Cargo.lock")
    packages = cargo_lock.get("package")
    if not isinstance(packages, list):
        raise ReleaseError("Cargo.lock must contain package entries")
    locked_versions = [
        item.get("version")
        for item in packages
        if isinstance(item, dict) and item.get("name") == "ultra-edit"
    ]
    if len(locked_versions) != 1:
        raise ReleaseError("Cargo.lock must contain exactly one ultra-edit package")
    locked_version = _require_semver(locked_versions[0], "Cargo.lock ultra-edit version")

    versions = {
        "Cargo.toml": cargo_version,
        "Cargo.lock": locked_version,
        "plugin.json": plugin_version,
    }
    if len(set(versions.values())) != 1:
        detail = ", ".join(f"{source}={version}" for source, version in versions.items())
        raise ReleaseError(f"release versions do not match: {detail}")
    return cargo_version


def verify_version(tag: str, repo_root: Path | None = None) -> str:
    tag_version = version_from_tag(tag)
    version = project_version(repo_root)
    if version != tag_version:
        raise ReleaseError(f"tag version {tag_version} does not match project version {version}")
    return version


def _validate_plugin_manifest(manifest: Mapping[str, object], source: str) -> str:
    if manifest.get("name") != "ultra-edit":
        raise ReleaseError(f"{source} must set name to 'ultra-edit'")
    version = _require_semver(manifest.get("version"), f"{source} version")
    description = manifest.get("description")
    if not isinstance(description, str) or not description.strip():
        raise ReleaseError(f"{source} must contain a non-empty description")
    author = manifest.get("author")
    if author is not None:
        if not isinstance(author, dict):
            raise ReleaseError(f"{source} author must be an object")
        if not isinstance(author.get("name"), str) or not author["name"].strip():
            raise ReleaseError(f"{source} author.name must be a non-empty string")
    if manifest.get("license") != DUAL_LICENSE:
        raise ReleaseError(f"{source} license must be {DUAL_LICENSE!r}")
    return version


def _validate_mcp_config(config: Mapping[str, object], source: str) -> None:
    servers = config.get("mcpServers")
    if not isinstance(servers, dict):
        raise ReleaseError(f"{source} must contain an mcpServers object")
    server = servers.get("ultra-edit")
    if not isinstance(server, dict):
        raise ReleaseError(f"{source} must define the ultra-edit MCP server")
    expected = {
        "type": "stdio",
        "command": "${CLAUDE_PLUGIN_ROOT}/runtime/ultra-edit-mcp",
        "args": ["--root", "${CLAUDE_PROJECT_DIR}"],
    }
    for key, value in expected.items():
        if server.get(key) != value:
            raise ReleaseError(f"{source} ultra-edit.{key} must be {value!r}")


def _validate_hooks_config(config: Mapping[str, object], source: str) -> None:
    hooks = config.get("hooks")
    if not isinstance(hooks, dict):
        raise ReleaseError(f"{source} must contain a hooks object")
    command = "${CLAUDE_PLUGIN_ROOT}/runtime/ultra-edit-mcp"
    for event in ("SessionStart", "SubagentStart"):
        groups = hooks.get(event)
        if not isinstance(groups, list) or len(groups) != 1 or not isinstance(groups[0], dict):
            raise ReleaseError(f"{source} must define exactly one {event} hook group")
        commands = groups[0].get("hooks")
        if not isinstance(commands, list) or len(commands) != 1 or not isinstance(commands[0], dict):
            raise ReleaseError(f"{source} must define exactly one {event} command hook")
        hook = commands[0]
        expected = {
            "type": "command",
            "command": command,
            "args": ["--claude-context", event],
            "timeout": 10,
        }
        for key, value in expected.items():
            if hook.get(key) != value:
                raise ReleaseError(f"{source} {event}.{key} must be {value!r}")


def _validate_plugin_files(files: Mapping[str, bytes], expected_version: str | None = None) -> str:
    required = (
        ".claude-plugin/plugin.json",
        ".mcp.json",
        "hooks/hooks.json",
        "instructions.md",
        "skills/edit/SKILL.md",
        *REQUIRED_REPO_DOCUMENTS,
    )
    missing = [name for name in required if name not in files]
    if missing:
        raise ReleaseError(f"plugin is missing required files: {', '.join(missing)}")

    manifest = _read_json_bytes(files[".claude-plugin/plugin.json"], ".claude-plugin/plugin.json")
    version = _validate_plugin_manifest(manifest, ".claude-plugin/plugin.json")
    if expected_version is not None and version != expected_version:
        raise ReleaseError(f"plugin version {version} does not match expected version {expected_version}")
    mcp = _read_json_bytes(files[".mcp.json"], ".mcp.json")
    _validate_mcp_config(mcp, ".mcp.json")
    hooks = _read_json_bytes(files["hooks/hooks.json"], "hooks/hooks.json")
    _validate_hooks_config(hooks, "hooks/hooks.json")
    return version


def _is_repo_document(name: str) -> bool:
    folded = name.casefold()
    if folded in EXACT_NOTICE_NAMES:
        return True
    if folded == "readme" or folded.startswith("readme."):
        return True
    return any(
        folded == prefix
        or folded.startswith(f"{prefix}.")
        or folded.startswith(f"{prefix}-")
        or folded.startswith(f"{prefix}_")
        for prefix in ("license", "copying")
    )


def _source_file_bytes(path: Path) -> bytes:
    try:
        data = path.read_bytes()
    except OSError as error:
        raise ReleaseError(f"cannot read release source file {path}: {error}") from error
    if path.name.casefold() in EXACT_NOTICE_NAMES:
        return data
    if path.suffix.casefold() in TEXT_SUFFIXES or _is_repo_document(path.name):
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ReleaseError(f"release text file is not valid UTF-8: {path}") from error
        return text.replace("\r\n", "\n").replace("\r", "\n").encode("utf-8")
    return data


def _source_plugin_files(repo_root: Path) -> dict[str, bytes]:
    plugin_root = repo_root / PLUGIN_PATH
    if not plugin_root.is_dir():
        raise ReleaseError(f"plugin directory does not exist: {plugin_root}")

    files: dict[str, bytes] = {}
    for path in sorted(plugin_root.rglob("*"), key=lambda item: item.as_posix()):
        relative = path.relative_to(plugin_root)
        if path.is_symlink():
            raise ReleaseError(f"plugin source cannot contain symlinks: {relative.as_posix()}")
        if relative.parts and relative.parts[0] == "runtime":
            continue
        if path.is_file():
            files[relative.as_posix()] = _source_file_bytes(path)

    for path in sorted(repo_root.iterdir(), key=lambda item: item.name):
        if path.is_symlink() and _is_repo_document(path.name):
            raise ReleaseError(f"repository document cannot be a symlink: {path.name}")
        if path.is_file() and _is_repo_document(path.name):
            existing = files.get(path.name)
            data = _source_file_bytes(path)
            if existing is not None and existing != data:
                raise ReleaseError(f"plugin file collides with repository document: {path.name}")
            files[path.name] = data

    missing_documents = [name for name in REQUIRED_REPO_DOCUMENTS if name not in files]
    if missing_documents:
        raise ReleaseError(
            "repository is missing required release documents: " + ", ".join(missing_documents)
        )

    _validate_plugin_files(files, project_version(repo_root))
    return files


def _safe_archive_name(name: str) -> None:
    if not name or "\\" in name or "\0" in name or name.startswith("/"):
        raise ReleaseError(f"unsafe archive entry name: {name!r}")
    raw_parts = name.split("/")
    if any(part in ("", ".", "..") for part in raw_parts):
        raise ReleaseError(f"unsafe archive entry name: {name!r}")
    path = PurePosixPath(name)
    if path.parts and ":" in path.parts[0]:
        raise ReleaseError(f"unsafe archive entry name: {name!r}")


def _write_zip(output: Path, entries: Mapping[str, tuple[bytes, int]]) -> None:
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    names = sorted(entries)
    folded: set[str] = set()
    for name in names:
        _safe_archive_name(name)
        collision_key = name.casefold()
        if collision_key in folded:
            raise ReleaseError(f"case-insensitive archive entry collision: {name}")
        folded.add(collision_key)

    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=output.parent, prefix=f".{output.name}.", suffix=".tmp", delete=False
        ) as temporary_file:
            temporary = Path(temporary_file.name)
        with zipfile.ZipFile(temporary, "w") as archive:
            for name in names:
                data, permissions = entries[name]
                info = zipfile.ZipInfo(name, FIXED_TIMESTAMP)
                info.create_system = 3
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = (stat.S_IFREG | permissions) << 16
                archive.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
        os.replace(temporary, output)
    except (OSError, zipfile.BadZipFile) as error:
        raise ReleaseError(f"cannot write archive {output}: {error}") from error
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def _read_archive(archive_path: Path) -> tuple[dict[str, bytes], dict[str, int]]:
    archive_path = Path(archive_path)
    try:
        if archive_path.stat().st_size > MAX_ARCHIVE_SIZE:
            raise ReleaseError(f"archive exceeds {MAX_ARCHIVE_SIZE} bytes: {archive_path}")
        with zipfile.ZipFile(archive_path) as archive:
            if archive.comment:
                raise ReleaseError("archive must not contain a comment")
            infos = archive.infolist()
            if not infos:
                raise ReleaseError("archive is empty")
            if len(infos) > MAX_ARCHIVE_ENTRIES:
                raise ReleaseError(f"archive contains more than {MAX_ARCHIVE_ENTRIES} entries")
            names = [info.filename for info in infos]
            if names != sorted(names):
                raise ReleaseError("archive entries are not sorted")

            total_size = sum(info.file_size for info in infos)
            if total_size > MAX_UNCOMPRESSED_SIZE:
                raise ReleaseError(
                    f"archive expands beyond the {MAX_UNCOMPRESSED_SIZE}-byte safety limit"
                )

            files: dict[str, bytes] = {}
            modes: dict[str, int] = {}
            folded: set[str] = set()
            for info in infos:
                name = info.filename
                _safe_archive_name(name)
                collision_key = name.casefold()
                if collision_key in folded:
                    raise ReleaseError(f"duplicate or case-colliding archive entry: {name}")
                folded.add(collision_key)
                if info.is_dir():
                    raise ReleaseError(f"archive must not contain directory entries: {name}")
                if info.flag_bits & 0x1:
                    raise ReleaseError(f"archive entry must not be encrypted: {name}")
                if info.date_time != FIXED_TIMESTAMP:
                    raise ReleaseError(f"archive entry has a non-deterministic timestamp: {name}")
                raw_mode = info.external_attr >> 16
                file_type = stat.S_IFMT(raw_mode)
                if file_type not in (0, stat.S_IFREG):
                    raise ReleaseError(f"archive entry is not a regular file: {name}")
                modes[name] = stat.S_IMODE(raw_mode)
                files[name] = archive.read(info)
            return files, modes
    except FileNotFoundError as error:
        raise ReleaseError(f"archive does not exist: {archive_path}") from error
    except (OSError, zipfile.BadZipFile, RuntimeError) as error:
        raise ReleaseError(f"cannot read archive {archive_path}: {error}") from error


def _runtime_names(files: Mapping[str, bytes]) -> set[str]:
    return {name for name in files if name.startswith("runtime/")}


def _validate_source_contents(files: Mapping[str, bytes], repo_root: Path) -> None:
    expected = _source_plugin_files(repo_root)
    actual = {name: data for name, data in files.items() if not name.startswith("runtime/")}
    missing = sorted(expected.keys() - actual.keys())
    unexpected = sorted(actual.keys() - expected.keys())
    changed = sorted(name for name in expected.keys() & actual.keys() if expected[name] != actual[name])
    if missing or unexpected or changed:
        details = []
        if missing:
            details.append(f"missing: {', '.join(missing)}")
        if unexpected:
            details.append(f"unexpected: {', '.join(unexpected)}")
        if changed:
            details.append(f"changed: {', '.join(changed)}")
        raise ReleaseError(f"archive plugin contents do not match the repository ({'; '.join(details)})")


def _require_modes(modes: Mapping[str, int], executable_names: set[str]) -> None:
    for name, mode in modes.items():
        expected = 0o755 if name in executable_names else 0o644
        if mode != expected:
            raise ReleaseError(f"archive entry {name} has mode {mode:04o}; expected {expected:04o}")


def _thin_runtime_names(target: Target) -> set[str]:
    return {f"runtime/{name}" for name in target.binary_names}


def _all_runtime_names() -> set[str]:
    names = {
        "runtime/ultra-edit.exe",
        "runtime/ultra-edit-mcp.exe",
        "runtime/ultra-edit",
        "runtime/ultra-edit-mcp",
    }
    for target in POSIX_TARGETS:
        names.update(
            f"runtime/targets/{target.triple}/{binary}" for binary in target.binary_names
        )
    return names


def verify_archive(
    archive_path: Path,
    *,
    target: str | None = None,
    all_targets: bool = False,
    repo_root: Path | None = None,
) -> str:
    if target is not None and all_targets:
        raise ReleaseError("choose either a target archive or an all-targets archive")
    if target is not None and target not in TARGETS:
        raise ReleaseError(f"unsupported target: {target}")

    root = _root(repo_root)
    files, modes = _read_archive(Path(archive_path))
    version = _validate_plugin_files(files, project_version(root))
    _validate_source_contents(files, root)
    runtime_names = _runtime_names(files)

    if all_targets or (target is None and any("/targets/" in name for name in runtime_names)):
        expected_runtime = _all_runtime_names()
    elif target is not None:
        expected_runtime = _thin_runtime_names(TARGETS[target])
    elif runtime_names == _thin_runtime_names(WINDOWS_TARGET):
        expected_runtime = _thin_runtime_names(WINDOWS_TARGET)
    elif runtime_names == {"runtime/ultra-edit", "runtime/ultra-edit-mcp"}:
        expected_runtime = runtime_names
    else:
        raise ReleaseError("cannot infer archive kind; pass --target or --all")

    if runtime_names != expected_runtime:
        missing = sorted(expected_runtime - runtime_names)
        unexpected = sorted(runtime_names - expected_runtime)
        details = []
        if missing:
            details.append(f"missing: {', '.join(missing)}")
        if unexpected:
            details.append(f"unexpected: {', '.join(unexpected)}")
        raise ReleaseError(f"archive runtime is incomplete ({'; '.join(details)})")
    _require_modes(modes, expected_runtime)
    return version


def package_thin(
    target_name: str,
    binary_dir: Path,
    output: Path,
    repo_root: Path | None = None,
) -> Path:
    target = TARGETS.get(target_name)
    if target is None:
        raise ReleaseError(f"unsupported target: {target_name}")
    root = _root(repo_root)
    version = project_version(root)
    output = Path(output)
    expected_name = f"ultra-edit-plugin-v{version}-{target_name}.zip"
    if output.name != expected_name:
        raise ReleaseError(f"target archive must be named {expected_name}")
    binary_dir = Path(binary_dir)
    if not binary_dir.is_dir():
        raise ReleaseError(f"binary directory does not exist: {binary_dir}")

    entries = {name: (data, 0o644) for name, data in _source_plugin_files(root).items()}
    for binary_name in target.binary_names:
        binary_path = binary_dir / binary_name
        if binary_path.is_symlink() or not binary_path.is_file():
            raise ReleaseError(f"required target binary does not exist: {binary_path}")
        data = binary_path.read_bytes()
        if not data:
            raise ReleaseError(f"target binary is empty: {binary_path}")
        entries[f"runtime/{binary_name}"] = (data, 0o755)

    _write_zip(output, entries)
    verify_archive(output, target=target_name, repo_root=root)
    return output


def _collect_thin_archives(input_dir: Path) -> tuple[str, dict[str, Path]]:
    input_dir = Path(input_dir)
    if not input_dir.is_dir():
        raise ReleaseError(f"thin archive directory does not exist: {input_dir}")
    versions: set[str] = set()
    archives: dict[str, Path] = {}
    for path in sorted(input_dir.iterdir(), key=lambda item: item.name):
        if not path.is_file():
            continue
        match = THIN_ARCHIVE_RE.fullmatch(path.name)
        if match is None:
            continue
        target = match.group("target")
        if target in archives:
            raise ReleaseError(f"multiple thin archives found for {target}")
        archives[target] = path
        versions.add(match.group("version"))

    missing = sorted(TARGETS.keys() - archives.keys())
    if missing:
        raise ReleaseError(
            "missing canonically named thin archives for: " + ", ".join(missing)
        )
    if len(versions) != 1:
        raise ReleaseError("thin archive filenames do not share one version")
    return next(iter(versions)), archives


def _launcher_bytes(repo_root: Path) -> bytes:
    path = repo_root / LAUNCHER_PATH
    try:
        data = path.read_bytes().replace(b"\r\n", b"\n")
    except OSError as error:
        raise ReleaseError(f"cannot read launcher {path}: {error}") from error
    if not data.startswith(b"#!/bin/sh\n") or b"\r" in data:
        raise ReleaseError("packaging/launcher.sh must be a Unix-format /bin/sh script")
    if not data.endswith(b"\n"):
        data += b"\n"
    return data


def package_all(input_dir: Path, output: Path, repo_root: Path | None = None) -> Path:
    root = _root(repo_root)
    expected_version = project_version(root)
    output = Path(output)
    expected_name = f"ultra-edit-plugin-v{expected_version}-all.zip"
    if output.name != expected_name:
        raise ReleaseError(f"all-target archive must be named {expected_name}")
    filename_version, archives = _collect_thin_archives(Path(input_dir))
    if filename_version != expected_version:
        raise ReleaseError(
            f"thin archive filename version {filename_version} does not match project version {expected_version}"
        )

    thin_files: dict[str, dict[str, bytes]] = {}
    for target_name, archive_path in archives.items():
        version = verify_archive(archive_path, target=target_name, repo_root=root)
        if version != filename_version:
            raise ReleaseError(
                f"{archive_path.name} contains plugin version {version}, expected {filename_version}"
            )
        thin_files[target_name], _ = _read_archive(archive_path)

    entries = {
        name: (data, 0o644)
        for name, data in thin_files[WINDOWS_TARGET.triple].items()
        if not name.startswith("runtime/")
    }
    for binary_name in WINDOWS_TARGET.binary_names:
        runtime_name = f"runtime/{binary_name}"
        entries[runtime_name] = (thin_files[WINDOWS_TARGET.triple][runtime_name], 0o755)

    launcher = _launcher_bytes(root)
    entries["runtime/ultra-edit"] = (launcher, 0o755)
    entries["runtime/ultra-edit-mcp"] = (launcher, 0o755)
    for target in POSIX_TARGETS:
        for binary_name in target.binary_names:
            source_name = f"runtime/{binary_name}"
            destination = f"runtime/targets/{target.triple}/{binary_name}"
            entries[destination] = (thin_files[target.triple][source_name], 0o755)

    _write_zip(output, entries)
    verify_archive(output, all_targets=True, repo_root=root)
    return output


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
    except OSError as error:
        raise ReleaseError(f"cannot hash {path}: {error}") from error
    return digest.hexdigest()


def _atomic_write(path: Path, data: bytes) -> None:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=path.parent, prefix=f".{path.name}.", suffix=".tmp", delete=False
        ) as output:
            temporary = Path(output.name)
            output.write(data)
        os.replace(temporary, path)
    except OSError as error:
        raise ReleaseError(f"cannot write {path}: {error}") from error
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def _marketplace_document(
    archive: Path, tag: str, repository: str, digest: str, repo_root: Path
) -> dict[str, object]:
    manifest = _read_json_file(repo_root / PLUGIN_PATH / ".claude-plugin/plugin.json")
    _validate_plugin_manifest(manifest, "plugin.json")
    owner = manifest.get("author")
    if not isinstance(owner, dict):
        owner = {"name": repository.split("/", 1)[0]}
    plugin: dict[str, object] = {
        "name": "ultra-edit",
        "source": {
            "source": "archive",
            "url": (
                f"https://github.com/{repository}/releases/download/"
                f"{quote(tag, safe='')}/{quote(archive.name, safe='')}"
            ),
            "sha256": digest,
        },
    }
    if isinstance(manifest.get("description"), str):
        plugin["description"] = manifest["description"]
    if isinstance(manifest.get("author"), dict):
        plugin["author"] = manifest["author"]
    return {
        "name": "ultra-edit",
        "description": manifest["description"],
        "owner": owner,
        "plugins": [plugin],
    }


def write_metadata(
    archive: Path,
    tag: str,
    repository: str,
    marketplace_output: Path,
    checksums_output: Path,
    asset_dir: Path,
    repo_root: Path | None = None,
) -> tuple[Path, Path]:
    root = _root(repo_root)
    version = verify_version(tag, root)
    if REPOSITORY_RE.fullmatch(repository) is None or any(
        part in (".", "..") for part in repository.split("/")
    ):
        raise ReleaseError("repository must have the form OWNER/REPO")

    archive = Path(archive)
    marketplace_output = Path(marketplace_output)
    checksums_output = Path(checksums_output)
    metadata_paths = {
        archive.resolve(),
        marketplace_output.resolve(),
        checksums_output.resolve(),
    }
    if len(metadata_paths) != 3:
        raise ReleaseError("archive, marketplace output, and checksums output must be distinct")

    expected_name = f"ultra-edit-plugin-v{version}-all.zip"
    if archive.name != expected_name:
        raise ReleaseError(f"all-target archive must be named {expected_name}")
    archive_version = verify_archive(archive, all_targets=True, repo_root=root)
    if archive_version != version:
        raise ReleaseError(f"archive version {archive_version} does not match tag version {version}")

    asset_dir = Path(asset_dir)
    if not asset_dir.is_dir():
        raise ReleaseError(f"asset directory does not exist: {asset_dir}")
    if archive.resolve().parent != asset_dir.resolve():
        raise ReleaseError("all-target archive must be a direct child of the asset directory")
    if marketplace_output.resolve().parent != asset_dir.resolve():
        raise ReleaseError("marketplace output must be a direct child of the asset directory")
    if checksums_output.resolve().parent != asset_dir.resolve():
        raise ReleaseError("checksums output must be a direct child of the asset directory")

    digest = _sha256(archive)
    marketplace = _marketplace_document(archive, tag, repository, digest, root)
    marketplace_bytes = (json.dumps(marketplace, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    _atomic_write(marketplace_output, marketplace_bytes)

    assets = []
    for path in asset_dir.iterdir():
        if path.resolve() == checksums_output.resolve():
            continue
        if path.is_symlink():
            raise ReleaseError(f"release asset cannot be a symlink: {path.name}")
        if path.is_file():
            if "\n" in path.name or "\r" in path.name:
                raise ReleaseError(f"release asset has an unsafe filename: {path.name!r}")
            assets.append(path)
    assets.sort(key=lambda item: item.name)
    if archive.resolve() not in {path.resolve() for path in assets}:
        raise ReleaseError("all-target archive is missing from the asset directory")
    checksum_bytes = "".join(f"{_sha256(path)}  {path.name}\n" for path in assets).encode("utf-8")
    _atomic_write(checksums_output, checksum_bytes)
    return marketplace_output, checksums_output


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    verify_version_parser = commands.add_parser(
        "verify-version", help="verify the release tag against all project manifests"
    )
    verify_version_parser.add_argument("--tag", required=True)

    thin_parser = commands.add_parser("package-thin", help="build one target-specific plugin ZIP")
    thin_parser.add_argument("--target", required=True, choices=tuple(TARGETS))
    thin_parser.add_argument("--binary-dir", required=True, type=Path)
    thin_parser.add_argument("--output", required=True, type=Path)

    all_parser = commands.add_parser("package-all", help="combine target ZIPs into one plugin ZIP")
    all_parser.add_argument("--input-dir", required=True, type=Path)
    all_parser.add_argument("--output", required=True, type=Path)

    metadata_parser = commands.add_parser(
        "write-metadata", help="write marketplace metadata and release checksums"
    )
    metadata_parser.add_argument("--archive", required=True, type=Path)
    metadata_parser.add_argument("--tag", required=True)
    metadata_parser.add_argument("--repository", required=True)
    metadata_parser.add_argument("--marketplace-output", required=True, type=Path)
    metadata_parser.add_argument("--checksums-output", required=True, type=Path)
    metadata_parser.add_argument("--asset-dir", required=True, type=Path)

    archive_parser = commands.add_parser("verify-archive", help="validate a packaged plugin ZIP")
    archive_parser.add_argument("--archive", required=True, type=Path)
    kind = archive_parser.add_mutually_exclusive_group()
    kind.add_argument("--target", choices=tuple(TARGETS))
    kind.add_argument("--all", action="store_true", dest="all_targets")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    try:
        if args.command == "verify-version":
            print(verify_version(args.tag))
        elif args.command == "package-thin":
            print(package_thin(args.target, args.binary_dir, args.output))
        elif args.command == "package-all":
            print(package_all(args.input_dir, args.output))
        elif args.command == "write-metadata":
            marketplace, checksums = write_metadata(
                args.archive,
                args.tag,
                args.repository,
                args.marketplace_output,
                args.checksums_output,
                args.asset_dir,
            )
            print(marketplace)
            print(checksums)
        elif args.command == "verify-archive":
            verify_archive(
                args.archive,
                target=args.target,
                all_targets=args.all_targets,
            )
            print(args.archive)
        else:  # pragma: no cover - argparse guarantees a known command.
            parser.error(f"unsupported command: {args.command}")
    except ReleaseError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
