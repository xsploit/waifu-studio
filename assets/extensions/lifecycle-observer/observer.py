#!/usr/bin/env python3
"""MIT-licensed example. Stdout is reserved for the host JSONL protocol."""
import json
import os
import sys
from pathlib import Path

data = Path(os.environ["WAIFU_EXTENSION_DATA"])
handshake = json.loads(sys.stdin.readline())
settings = handshake["message"].get("settings", {})
(data / "startup.json").write_text(json.dumps({"session_id": handshake["session_id"], "settings": settings}))
receipt = data / "latest-event.json"
receipt.write_text(json.dumps({"session_id": handshake["session_id"], "state": "started"}))
events_seen = 0
for line in sys.stdin:
    envelope = json.loads(line)
    message = envelope.get("message", {})
    if message.get("type") == "event" and settings.get("record_events", True):
        event = message["event"]
        if settings.get("event_group") == "speech" and not event.get("type", "").startswith("speech."):
            continue
        events_seen += 1
        if events_seen % settings.get("every_n_events", 1):
            continue
        # Replace one bounded receipt rather than accumulating a log forever.
        receipt.write_text(json.dumps(message["event"]))
