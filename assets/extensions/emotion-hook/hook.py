#!/usr/bin/env python3
"""MIT-licensed one-shot metadata receipt. Stdout is reserved for host JSONL."""
import json
import os
from pathlib import Path
import sys
from uuid import UUID

MAX_LINE_BYTES = 64 * 1024
MAX_RECEIPT_BYTES = 1024


def read_host():
    line = sys.stdin.readline(MAX_LINE_BYTES + 1)
    if not line:
        raise ValueError("Missing host packet")
    if len(line.encode("utf-8")) > MAX_LINE_BYTES:
        raise ValueError("Host packet exceeds limit")
    envelope = json.loads(line)
    if envelope.get("api_version") != 1 or envelope.get("type") != "host":
        raise ValueError("Invalid host packet")
    return envelope


def valid_uuid(value):
    if not isinstance(value, str) or len(value) != 36 or UUID(value).int == 0:
        raise ValueError("Invalid source identity")


def bounded_name(value):
    if not isinstance(value, str) or not 1 <= len(value) <= 32:
        raise ValueError("Invalid emotion name")
    if not all("a" <= char <= "z" for char in value):
        raise ValueError("Invalid emotion name")


def run():
    handshake = read_host()
    if handshake["message"]["type"] != "handshake":
        raise ValueError("Missing handshake")
    if "lifecycle.observe" not in handshake["message"]["capabilities"]:
        raise ValueError("Missing lifecycle permission")
    while True:
        envelope = read_host()
        if (envelope["session_id"] != handshake["session_id"]
                or envelope["process_generation"] != handshake["process_generation"]):
            raise ValueError("Host identity changed")
        message = envelope["message"]
        if message["type"] != "event":
            continue
        event = message["event"]
        if event["type"] != "emotion.changed":
            continue
        valid_uuid(event["character"])
        if event["owner"] not in ("studio", "scene_chat"):
            raise ValueError("Invalid source owner")
        bounded_name(event["previous"])
        bounded_name(event["emotion"])
        if event.get("reply_id") is not None:
            valid_uuid(event["reply_id"])
        # Preserve the original source identity; never consult target.changed.
        receipt = {key: event[key] for key in
                   ("type", "character", "owner", "previous", "emotion")}
        receipt["reply_id"] = event.get("reply_id")
        encoded = json.dumps(receipt, ensure_ascii=True, separators=(",", ":")) + "\n"
        if len(encoded.encode("utf-8")) > MAX_RECEIPT_BYTES:
            raise ValueError("Receipt exceeds limit")
        (Path(os.environ["WAIFU_EXTENSION_DATA"]) / "latest.json").write_text(
            encoded, encoding="utf-8")
        return


if __name__ == "__main__":
    try:
        run()
    except (ValueError, KeyError, TypeError, OSError):
        # Never print packet contents, private values, or raw exception text.
        print("Emotion hook could not record metadata", file=sys.stderr)
        sys.exit(1)
