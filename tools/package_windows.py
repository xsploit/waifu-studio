#!/usr/bin/env python3
"""Package an explicitly built Windows x64 executable with its runtime assets."""
import argparse
import json
from pathlib import Path
import shutil
import struct
import tempfile
import zipfile

from package_linux import MANIFEST, copy_runtime, digest, inventory, verify, release_metadata


def build(repo, binary, output, release=None):
    repo, binary, output = Path(repo).resolve(), Path(binary).resolve(), Path(output).absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("Refusing to replace an existing package")
    metadata = release_metadata(repo, release)
    with binary.open("rb") as stream:
        header = stream.read(64)
        if len(header) != 64 or header[:2] != b"MZ":
            raise ValueError("Expected a Windows x64 PE executable")
        stream.seek(struct.unpack_from("<I", header, 60)[0])
        if stream.read(6) != b"PE\0\0\x64\x86":
            raise ValueError("Expected a Windows x64 PE executable")
    output.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix="waifu-windows-", dir=output.parent))
    try:
        copy_runtime(repo, binary, stage, platform="windows-x86_64")
        (stage / "waifu-studio").rename(stage / "waifu-studio.exe")
        (stage / "start.cmd").write_text('@echo off\n"%~dp0waifu-studio.exe" %*\n', encoding="utf-8")
        (stage / "studio.cmd").write_text('@echo off\n"%~dp0waifu-studio.exe" --studio %*\n', encoding="utf-8")
        (stage / "README.txt").write_text(
            f"Waifu Studio - {release or 'private candidate'} - Windows x64\n\n"
            "Run studio.cmd for setup, or start.cmd for the desktop companion.\n"
            "Keep waifu-studio.exe beside assets/. No Rust installation is needed.\n"
            "Windows 10/11 x64 and a current graphics driver are required.\n"
            "Install Microsoft's Visual C++ x64 runtime if VCRUNTIME140.dll is missing.\n"
            "User data is separate under %LOCALAPPDATA%\\waifu-studio\\data.\n"
            "Python integrations need Python 3 with the py launcher; JavaScript\n"
            "integrations need Node.js. These interpreters are not bundled.\n"
            "Silero speech detection and its CPU runtime are included.\n"
            "See docs/CROSS-PLATFORM.md for validation and graphics guidance.\n\n"
            "This is an experimental tester build. See docs/PREVIEW.md.\n"
            "Bundled model metadata and attribution are retained under assets/.\n"
            "Live services require your own accounts and configuration.\n",
            encoding="utf-8",
        )
        data = {"format_version": 1, **metadata, "platform": "windows-x86_64", "binary_sha256": digest(binary), "files": inventory(stage)}
        (stage / MANIFEST).write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        verify(stage)
        stage.rename(output)
        return data
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def archive(root, destination):
    root, destination = Path(root), Path(destination)
    verify(root)
    with zipfile.ZipFile(destination, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as zipped:
        for path in sorted(root.rglob("*")):
            if path.is_file():
                zipped.write(path, (Path(root.name) / path.relative_to(root)).as_posix())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--release", help="Version tag; requires committed source inputs")
    args = parser.parse_args()
    data = build(args.repo, args.binary, args.output, args.release)
    if args.archive:
        archive(args.output, args.archive)
    print(json.dumps({"files": len(data["files"]), "binary_sha256": data["binary_sha256"]}, indent=2))


if __name__ == "__main__":
    main()
