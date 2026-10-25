#!/usr/bin/env python3
"""MIT metadata-only integration startup observer; no actions, networking or reply text."""
import json
import os
from pathlib import Path
import select
import sys
import time
from uuid import UUID


def unique_fields(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate field")
        value[key] = item
    return value


def main():
    # Bounded raw reads avoid text-buffer/select interactions and partial-line hangs.
    deadline = time.monotonic() + 4.0
    pending = bytearray()
    identity = None
    sequence = 0
    packets = 0
    while time.monotonic() < deadline:
        if b"\n" not in pending:
            if not select.select([sys.stdin], [], [], max(0, deadline - time.monotonic()))[0]:
                return 1
            block = os.read(sys.stdin.fileno(), 4096)
            if not block:
                return 1
            pending.extend(block)
            if len(pending) > 65536:
                return 1
            continue
        line, _, tail = pending.partition(b"\n")
        pending = bytearray(tail)
        packets += 1
        if packets > 16:
            return 1
        packet = json.loads(line, object_pairs_hook=unique_fields)
        session = packet["session_id"]
        generation = packet["process_generation"]
        if not isinstance(session, str):
            return 1
        parsed_session = UUID(session)
        if parsed_session.int == 0 or str(parsed_session) != session:
            return 1
        current = (session, generation)
        if (type(packet.get("api_version")) is not int
                or packet["api_version"] != 1 or packet.get("type") != "host"
                or type(generation) is not int or not 0 < generation <= (1 << 64) - 1
                or type(packet.get("sequence")) is not int
                or packet["sequence"] <= sequence):
            return 1
        sequence = packet["sequence"]
        message = packet["message"]
        if identity is None:
            if (message.get("type") != "handshake"
                    or message.get("capabilities") != ["lifecycle.observe"]):
                return 1
            identity = current
            continue
        if current != identity:
            return 1
        if message.get("type") != "event":
            return 1
        event = message["event"]
        if event.get("type") == "target.changed":
            # Integration startup metadata never supplies avatar-control authority.
            if event.get("target") is not None:
                return 1
            continue
        if (set(event) != {"type", "installation_id", "session_id", "process_generation"}
                or event["type"] != "extension.started"):
            return 1
        for key in ("installation_id", "session_id"):
            value = event[key]
            if not isinstance(value, str):
                return 1
            parsed = UUID(value)
            if parsed.int == 0 or str(parsed) != value:
                return 1
        generation = event["process_generation"]
        if type(generation) is not int or not 0 < generation <= (1 << 64) - 1:
            return 1
        destination = Path(os.environ["WAIFU_EXTENSION_DATA"]) / "latest-integration-start.json"
        temporary = destination.with_suffix(".tmp")
        temporary.write_text(json.dumps(event) + "\n", encoding="utf-8")
        temporary.replace(destination)
        return 0
    return 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, TypeError, ValueError, OSError, RecursionError):
        sys.exit(1)
