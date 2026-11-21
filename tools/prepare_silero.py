#!/usr/bin/env python3
"""Prepare pinned, verified Silero CPU assets for source builds and release packages."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import sys
import urllib.request
import zipfile

MODEL_COMMIT = "7e30209a3e901f9842f81b225f3e93d8199902b1"
MODEL_BASE = f"https://raw.githubusercontent.com/snakers4/silero-vad/{MODEL_COMMIT}/"
ORT_VERSION = "1.24.4"
ORT_PACKAGE = "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime"
ORT_URL = f"{ORT_PACKAGE}/{ORT_VERSION}/microsoft.ml.onnxruntime.{ORT_VERSION}.nupkg"
ORT_SHA256 = "4b978d5065b85e7004b6c6f60ca494bd978fbe6836cbf0a0b52d82b61ab99638"
RUNTIMES = {
    "windows-x86_64": ("win-x64", ("onnxruntime.dll", "onnxruntime_providers_shared.dll")),
    "linux-x86_64": ("linux-x64", ("libonnxruntime.so", "libonnxruntime_providers_shared.so")),
}
NOTICES = ("LICENSE", "ThirdPartyNotices.txt", "Privacy.md")


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
    data = fetch(ORT_URL, ORT_SHA256, cache)
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        notices = {name: archive.read(name) for name in NOTICES}
        for platform in platforms:
            target, libraries = RUNTIMES[platform]
            files = {name: archive.read(f"runtimes/{target}/native/{name}") for name in libraries}
            files.update(notices)
            for name, content in files.items():
                write(output / platform / name, content)
            receipt = {"silero_version": "6.2.1", "silero_commit": MODEL_COMMIT, "onnxruntime_version": ORT_VERSION,
                       "archive_url": ORT_URL, "archive_sha256": ORT_SHA256,
                       "files": {name: sha256(content) for name, content in sorted(files.items())}}
            write(output / platform / "provenance.json", (json.dumps(receipt, indent=2) + "\n").encode())
            print(f"Prepared Silero 6.2.1 and ONNX Runtime {ORT_VERSION}: {platform}")


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=[*RUNTIMES, "all"], default="windows-x86_64" if sys.platform == "win32" else "linux-x86_64")
    parser.add_argument("--output", type=Path, default=repo / "assets/silero")
    parser.add_argument("--cache", type=Path, default=repo / "work/silero-downloads")
    args = parser.parse_args()
    prepare(args.output, args.cache, list(RUNTIMES) if args.platform == "all" else [args.platform])


if __name__ == "__main__":
    main()
