#!/usr/bin/env python3
"""Offline saved-reaction preview. No network, credentials, or message delivery."""
import json
import os
from pathlib import Path
import select
import sys
import time
from uuid import UUID

DEFAULT_MOTION = "studio.integration.twitch/nod"
MAX_LINE = 64 * 1024


class Abort(Exception):
    pass


def reserved(name):
    name = name.lower()
    return name.startswith(("blink", "look", "mouth")) or name in (
        "aa", "ih", "ou", "ee", "oh", "a", "i", "u", "e", "o")


def invalid_text(value, limit):
    return (not isinstance(value, str) or len(value.encode("utf-8")) > limit
            or any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value))


def uuid_ok(value):
    return isinstance(value, str) and len(value) == 36 and UUID(value).int != 0


class Preview:
    def __init__(self):
        self.receipt = Path(os.environ["WAIFU_EXTENSION_DATA"]) / "latest-test.json"
        self.deadline = time.monotonic() + 9.0
        self.buffer = bytearray()
        self.session = None
        self.generation = None
        self.target = None
        self.sequence = 0
        self.pending = None
        self.stage = None
        self.expression_until = None
        self.finished = False

    def record(self, status):
        # Fixed status vocabulary, no packet contents, settings, or private text.
        encoded = json.dumps({"state": status}, separators=(",", ":")) + "\n"
        if len(encoded) > 1024:
            raise Abort()
        temporary = self.receipt.with_suffix(".tmp")
        temporary.write_text(encoded, encoding="utf-8")
        temporary.replace(self.receipt)

    def packet(self, until):
        while True:
            newline = self.buffer.find(b"\n")
            if newline >= 0:
                if newline + 1 > MAX_LINE:
                    raise Abort()
                line = bytes(self.buffer[:newline + 1])
                del self.buffer[:newline + 1]
                return json.loads(line)
            if len(self.buffer) >= MAX_LINE:
                raise Abort()
            remaining = min(until, self.deadline) - time.monotonic()
            if remaining <= 0:
                return None
            if not select.select([sys.stdin], [], [], remaining)[0]:
                return None
            chunk = os.read(sys.stdin.fileno(), 4096)
            if not chunk:
                raise Abort()
            self.buffer.extend(chunk)

    def send(self, stage, action):
        self.sequence += 1
        self.pending = "reaction-test-" + str(self.sequence)
        self.stage = stage
        print(json.dumps({"api_version": 1, "type": "action", "id": self.pending,
              "sequence": self.sequence, "session_id": self.session,
              "process_generation": self.generation, "target": self.target,
              "action": action}), flush=True)
        self.record(stage + "_pending")

    def start_motion(self):
        if self.motion:
            self.send("motion", {"type": "motion.play", "motion_id": self.motion,
                                 "mode": "one_shot"})
        else:
            self.finished = True
            self.record("expression_accepted")

    def run(self):
        self.record("waiting")
        handshake = self.packet(self.deadline)
        if handshake is None or handshake.get("api_version") != 1 or handshake.get("type") != "host":
            raise Abort()
        self.session = handshake["session_id"]
        self.generation = handshake["process_generation"]
        if not uuid_ok(self.session) or type(self.generation) is not int or self.generation < 1:
            raise Abort()
        message = handshake["message"]
        if message["type"] != "handshake":
            raise Abort()
        grants = message["capabilities"]
        if "lifecycle.observe" not in grants:
            self.record("observation_required")
            return 1
        settings = message.get("settings", {})
        expression = settings.get("celebration_expression", "")
        motion = settings.get("celebration_motion", DEFAULT_MOTION)
        if (invalid_text(expression, 128) or (expression and not expression.strip())
                or reserved(expression) or invalid_text(motion, 193)
                or any(c.isspace() for c in motion)):
            self.record("invalid_settings")
            return 1
        self.expression = expression if "expression.apply" in grants else ""
        self.motion = motion if "motion.play" in grants else ""
        if not self.expression and not self.motion:
            self.record("nothing_selected_or_permitted")
            return
        while True:
            now = time.monotonic()
            if now >= self.deadline:
                self.record("timed_out")
                return 1
            if self.finished and (self.expression_until is None or now >= self.expression_until):
                self.record("completed")
                return
            until = self.expression_until if self.finished else self.deadline
            envelope = self.packet(until)
            if envelope is None:
                continue
            if (envelope.get("api_version") != 1 or envelope.get("type") != "host"
                    or envelope["session_id"] != self.session
                    or envelope["process_generation"] != self.generation):
                raise Abort()
            message = envelope["message"]
            if message["type"] == "event":
                event = message["event"]
                if event["type"] == "target.changed":
                    target = event["target"]
                    if self.target is not None:
                        if target != self.target:
                            self.record("target_changed")
                            return 1
                    elif (not isinstance(target, dict) or not uuid_ok(target["uuid"])
                          or type(target["generation"]) is not int or target["generation"] < 0):
                        self.record("no_target")
                        return 1
                    else:
                        self.target = dict(target)
                        if self.expression:
                            self.send("expression", {"type": "expression.apply", "name": self.expression,
                                      "strength": 0.75, "duration_ms": 2000})
                        else:
                            self.start_motion()
                elif (event["type"] == "motion.finished" and self.stage == "motion"
                      and event["action_id"] == self.pending and event["target"] == self.target
                      and event["motion_id"] == self.motion):
                    if event["outcome"] != "completed":
                        self.record("motion_not_completed")
                        return 1
                    self.pending = None
                    self.stage = None
                    self.finished = True
                    self.record("motion_completed")
            elif (message["type"] == "result" and message["id"] == self.pending
                  and message["target"] == self.target):
                if message["result"]["status"] != "ok":
                    self.record("rejected")
                    return 1
                if self.stage == "expression":
                    self.pending = None
                    self.stage = None
                    self.expression_until = time.monotonic() + 2.0
                    self.start_motion()
                # Motion acceptance is not a terminal receipt.


if __name__ == "__main__":
    preview = None
    try:
        preview = Preview()
        sys.exit(preview.run())
    except (Abort, ValueError, KeyError, TypeError, OSError, UnicodeError):
        if preview is not None:
            preview.record("aborted")
        print("Offline reaction test stopped", file=sys.stderr)
        sys.exit(1)
