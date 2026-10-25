#!/usr/bin/env python3
"""Prepare pinned, verified Silero CPU assets for source builds and release packages."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import sys
import tarfile
import urllib.request
import zipfile

MODEL_COMMIT = "7e30209a3e901f9842f81b225f3e93d8199902b1"
MODEL_BASE = f"https://raw.githubusercontent.com/snakers4/silero-vad/{MODEL_COMMIT}/"
ORT_VERSION = "1.24.4"
ARCHIVES = {
    "windows-x86_64": ("onnxruntime-win-x64-1.24.4.zip", "d2319fddfb6ea4db99ccc4b60c85c517bcd855721f5daa6a06d40d7cb2ee2357"),
    "linux-x86_64": ("onnxruntime-linux-x64-1.24.4.tgz", "3a211fbea252c1e66290658f1b735b772056149f28321e71c308942cdb54b747"),
}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def fetch(url, expected, cache):
    path = cache / expected
    if path.is_file():
        data = path.read_bytes()
        if sha256(data) == expected:
            return data
    with urllib.request.urlopen(url, timeout=60) as response:
        data = response.read(128 * 1024 * 1024 + 1)
    if len(data) > 128 * 1024 * 1024 or sha256(data) != expected:
        raise ValueError("Downloaded asset did not match its pinned SHA-256")
    path.write_bytes(data)
    return data


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".partial")
    temporary.write_bytes(data)
    temporary.replace(path)
    path.chmod(0o644)


def prepare(output, cache, platforms):
    output.mkdir(parents=True, exist_ok=True)
    cache.mkdir(parents=True, exist_ok=True)
    for source, name, expected in [
        ("src/silero_vad/data/silero_vad.onnx", "silero_vad.onnx", "1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3"),
        ("LICENSE", "SILERO-LICENSE.txt", "2e63e9a38b6e8fc0c7bc37ce174caca1862870856c6daf5697cfb785e925520b"),
    ]:
        write(output / name, fetch(MODEL_BASE + source, expected, cache))
    for platform in platforms:
        archive_name, expected = ARCHIVES[platform]
        url = f"https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}/{archive_name}"
        data = fetch(url, expected, cache)
        files = {}
        if platform.startswith("windows"):
            with zipfile.ZipFile(io.BytesIO(data)) as archive:
                for member in archive.infolist():
                    name = Path(member.filename).name
                    if name in {"onnxruntime.dll", "onnxruntime_providers_shared.dll", "LICENSE", "ThirdPartyNotices.txt", "Privacy.md"}:
                        files[name] = archive.read(member)
            required = "onnxruntime.dll"
        else:
            with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
                for member in archive.getmembers():
                    name = Path(member.name).name
                    if member.isfile() and name in {f"libonnxruntime.so.{ORT_VERSION}", "libonnxruntime_providers_shared.so", "LICENSE", "ThirdPartyNotices.txt", "Privacy.md"}:
                        files["libonnxruntime.so" if name.startswith("libonnxruntime.so.") else name] = archive.extractfile(member).read()
            required = "libonnxruntime.so"
        if required not in files or "LICENSE" not in files:
            raise ValueError("Runtime archive is missing its library or license")
        for name, content in files.items():
            write(output / platform / name, content)
        receipt = {"silero_version": "6.2.1", "silero_commit": MODEL_COMMIT, "onnxruntime_version": ORT_VERSION,
                   "archive_url": url, "archive_sha256": expected,
                   "files": {name: sha256(content) for name, content in sorted(files.items())}}
        write(output / platform / "provenance.json", (json.dumps(receipt, indent=2) + "\n").encode())
        print(f"Prepared Silero 6.2.1 and ONNX Runtime {ORT_VERSION}: {platform}")


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=[*ARCHIVES, "all"], default="windows-x86_64" if sys.platform == "win32" else "linux-x86_64")
    parser.add_argument("--output", type=Path, default=repo / "assets/silero")
    parser.add_argument("--cache", type=Path, default=repo / "work/silero-downloads")
    args = parser.parse_args()
    prepare(args.output, args.cache, list(ARCHIVES) if args.platform == "all" else [args.platform])


if __name__ == "__main__":
    main()
