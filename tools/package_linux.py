#!/usr/bin/env python3
"""Build a Linux candidate or tagged preview from runtime inputs, never a profile."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

MANIFEST = "package-manifest.json"


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def regular_files(root):
    if root.is_symlink() or not root.is_dir():
        raise ValueError("Expected an ordinary directory")
    for directory, folders, files in os.walk(root, followlinks=False):
        for name in folders + files:
            path = Path(directory) / name
            if path.is_symlink() or name.startswith("."):
                raise ValueError(f"Links and hidden entries are excluded: {path.name}")
            if path.is_file():
                if name.startswith("credentials") or name.endswith((".sqlite", ".db", ".env")):
                    raise ValueError("Profile/credential files cannot enter a runtime package")
                yield path
            elif not path.is_dir():
                raise ValueError("Non-regular package entry")


def inventory(root):
    return {
        path.relative_to(root).as_posix(): {
            "sha256": digest(path),
            "bytes": path.stat().st_size,
            "executable": bool(path.stat().st_mode & 0o111),
        }
        for path in sorted(regular_files(root))
        if path.relative_to(root).as_posix() != MANIFEST
    }


def verify(root):
    root = Path(root)
    manifest = root / MANIFEST
    if manifest.is_symlink():
        raise ValueError("Package manifest must be an ordinary file")
    data = json.loads(manifest.read_text())
    preview = data.get("private") is False and valid_release(data.get("release")) and re.fullmatch(r"[0-9a-f]{40}", data.get("source_commit", ""))
    if data.get("format_version") != 1 or not (data.get("private") is True or preview):
        raise ValueError("Unsupported package manifest")
    if inventory(root) != data.get("files"):
        raise ValueError("Package file inventory, hash, size or executable state differs")
    return data


def valid_release(value):
    return isinstance(value, str) and re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?", value) is not None


def release_metadata(repo, release):
    if release is None:
        return {"private": True}
    if not valid_release(release):
        raise ValueError("Release must be a version tag such as v0.1.0-preview.1")
    commit = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
    subprocess.run(["git", "-C", str(repo), "diff", "--exit-code", "--quiet", "HEAD", "--"], check=True)
    if subprocess.check_output(["git", "-C", str(repo), "ls-files", "--others", "--exclude-standard"], text=True).strip():
        raise ValueError("Commit all release inputs before creating a tagged preview")
    return {"private": False, "release": release, "source_commit": commit}


def copy_runtime(repo, binary, root):
    # These are source-controlled runtime trees, never application data dirs.
    plan = [(binary, Path("waifu-studio"))]
    for name in ("assets", "docs"):
        for path in regular_files(repo / name):
            plan.append((path, Path(name) / path.relative_to(repo / name)))
    for name in ("LICENSE", "REFERENCES.md", "FEATURE-STATUS.md", "VALIDATION.md"):
        plan.append((repo / name, Path(name)))
    for path in regular_files(repo / "vendor"):
        if path.name.startswith("LICENSE") or path.name == "WAIFU-PATCH.md":
            plan.append((path, path.relative_to(repo)))
    for source, relative in plan:
        if source.is_symlink() or not source.is_file():
            raise ValueError("Runtime inputs must be regular files")
        before = digest(source)
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        # NTFS mounted through WSL reports every source file as executable.
        # Runtime entrypoints need execute permission; models/docs do not.
        with source.open("rb") as stream:
            script = stream.read(3) == b"#!/"
        target.chmod(0o755 if source == binary or script else 0o644)
        if digest(target) != before:
            raise ValueError("Runtime input changed during copying")


def build(repo, binary, output, release=None):
    repo, binary, output = Path(repo).resolve(), Path(binary).resolve(), Path(output).absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("Refusing to replace an existing package")
    metadata = release_metadata(repo, release)
    with binary.open("rb") as stream:
        header = stream.read(20)
    if header[:6] != b"\x7fELF\x02\x01" or header[18:20] != b"\x3e\x00":
        raise ValueError("Expected the built Linux x86_64 ELF executable")
    output.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix="waifu-package-", dir=output.parent))
    try:
        copy_runtime(repo, binary, stage)
        (stage / "start.sh").write_text(
            '#!/bin/sh\nset -eu\n'
            'package_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\n'
            'cd -- "$package_dir"\nexec "$package_dir/waifu-studio" "$@"\n'
        )
        (stage / "start.sh").chmod(0o755)
        (stage / "studio.sh").write_text('#!/bin/sh\nset -eu\npackage_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\nexec "$package_dir/start.sh" --studio "$@"\n')
        (stage / "studio.sh").chmod(0o755)
        (stage / "README.txt").write_text(
            f"Waifu Studio — {release or 'private candidate'} — Linux x86_64\n\n"
            "Run ./studio.sh for setup, or ./start.sh for the desktop companion.\n"
            "Rust and Cargo are not needed to launch.\n"
            "Keep the executable and assets directory together. User profiles and\n"
            "credentials are stored separately and are not included here.\n"
            "Requires the system libraries recorded in DEPENDENCIES.txt, a working\n"
            "graphics driver and a Wayland or X11 desktop. Other Linux distributions\n"
            "and platforms are unverified. Python/Node examples require their named\n"
            "interpreters; executable integrations remain opt-in.\n\n"
            "This is an experimental tester build. See docs/PREVIEW.md for setup and known issues.\n"
            "Bundled model metadata and attribution are retained under assets/.\n"
            "No experimental generated motion library, accounts or keys are included.\n"
            "Motion quality, live services and long-session acceptance remain open.\n"
        )
        dependencies = subprocess.run(["ldd", str(binary)], capture_output=True, text=True, check=True)
        if "not found" in dependencies.stdout:
            raise ValueError("Built executable has unresolved system dependencies")
        versions = subprocess.run(["readelf", "--version-info", str(binary)], capture_output=True, text=True, check=True)
        (stage / "DEPENDENCIES.txt").write_text(
            "Observed on the build host; not proof of cross-distribution support.\n\n"
            + dependencies.stdout + "\nELF symbol version requirements:\n" + versions.stdout
        )
        data = {"format_version": 1, **metadata, "platform": "linux-x86_64", "binary_sha256": digest(binary), "files": inventory(stage)}
        (stage / MANIFEST).write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")
        verify(stage)
        stage.rename(output)
        return data
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def archive(root, destination):
    root, destination = Path(root), Path(destination)
    verify(root)
    if destination.exists() or destination.is_symlink():
        raise ValueError("Refusing to replace an existing archive")
    with destination.open("xb") as stream:
        with tarfile.open(fileobj=stream, mode="w:gz") as tar:
            tar.add(root, arcname=root.name)
    return digest(destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify", type=Path)
    parser.add_argument("--repo", type=Path)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--release", help="Version tag; requires committed source inputs")
    args = parser.parse_args()
    if args.verify:
        data = verify(args.verify)
        root = args.verify
    else:
        if not all((args.repo, args.binary, args.output)):
            parser.error("build requires --repo, --binary and --output")
        data = build(args.repo, args.binary, args.output, args.release)
        root = args.output
    result = {"files": len(data["files"]), "bytes": sum(x["bytes"] for x in data["files"].values()), "binary_sha256": data["binary_sha256"]}
    if args.archive:
        result["archive_sha256"] = archive(root, args.archive)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
