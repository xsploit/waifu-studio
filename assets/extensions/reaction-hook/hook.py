#!/usr/bin/env python3
"""MIT one-shot API-v1 reaction; stdout contains only action envelopes."""
import json
import os
from pathlib import Path
import select
import sys
import time
from uuid import UUID

MAX_LINE = 64 * 1024
U64_MAX = (1 << 64) - 1
EVENTS = {"Reply finished": "reply.finished", "Speech started": "speech.started",
          "Speech stopped": "speech.stopped", "Emotion changed": "emotion.changed"}
CAPABILITIES = {"lifecycle.observe", "motion.play", "expression.apply",
                "message.deliver", "event.emit", "catalog.get", "reply.read"}


class Abort(Exception):
    pass


def require(condition):
    if not condition:
        raise Abort()


def uint(value, positive=False):
    return type(value) is int and int(positive) <= value <= U64_MAX


def canonical_uuid(value):
    if not isinstance(value, str) or len(value) != 36:
        return False
    try:
        parsed = UUID(value)
        return parsed.int != 0 and str(parsed) == value
    except ValueError:
        return False


def text(value, limit):
    return (isinstance(value, str) and len(value.encode("utf-8")) <= limit
            and not any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value))


def reserved(value):
    value = value.lower()
    return value.startswith(("blink", "look", "mouth")) or value in (
        "aa", "ih", "ou", "ee", "oh", "a", "i", "u", "e", "o")


def object_pairs(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result)
        result[key] = value
    return result


def reject_constant(_):
    raise Abort()


def target_ok(value):
    return (isinstance(value, dict) and set(value) == {"uuid", "generation"}
            and canonical_uuid(value["uuid"]) and uint(value["generation"]))


class Reaction:
    def __init__(self, testing):
        self.testing = testing
        self.receipt = Path(os.environ["WAIFU_EXTENSION_DATA"]) / (
            "latest-test.json" if testing else "latest-hook.json")
        self.deadline = time.monotonic() + 9.0  # Within the host's ten-second cap.
        self.buffer = bytearray()
        self.session = None
        self.generation = None
        self.host_sequence = 0
        self.sequence = 0
        self.packets = 0
        self.target = None
        self.source = None
        self.started = False
        self.expression_id = None
        self.motion_id = None
        self.motion_accepted = False
        self.motion_completed = False
        self.expression_until = None

    def record(self, state):
        # No source text, settings, credentials or arbitrary host errors on disk.
        temporary = self.receipt.with_suffix(".tmp")
        temporary.write_text(json.dumps({"state": state}) + "\n", encoding="utf-8")
        temporary.replace(self.receipt)

    def packet(self, until):
        while True:
            newline = self.buffer.find(b"\n")
            if newline >= 0:
                require(newline + 1 <= MAX_LINE)
                line = bytes(self.buffer[:newline + 1])
                del self.buffer[:newline + 1]
                self.packets += 1
                require(self.packets <= 1024)
                packet = json.loads(line, object_pairs_hook=object_pairs,
                                    parse_constant=reject_constant)
                require(isinstance(packet, dict) and type(packet.get("api_version")) is int
                        and packet["api_version"] == 1 and packet.get("type") == "host"
                        and canonical_uuid(packet.get("session_id"))
                        and uint(packet.get("process_generation"), True)
                        and uint(packet.get("sequence"), True)
                        and packet["sequence"] > self.host_sequence
                        and isinstance(packet.get("message"), dict))
                if self.session is not None:
                    require(packet["session_id"] == self.session
                            and packet["process_generation"] == self.generation)
                self.host_sequence = packet["sequence"]
                return packet
            require(len(self.buffer) < MAX_LINE)
            remaining = min(until, self.deadline) - time.monotonic()
            if remaining <= 0 or not select.select([sys.stdin], [], [], remaining)[0]:
                return None
            chunk = os.read(sys.stdin.fileno(), 4096)
            require(bool(chunk))
            self.buffer.extend(chunk)

    def send(self, action):
        self.sequence += 1
        action_id = "reaction-" + str(self.sequence)
        print(json.dumps({"api_version": 1, "type": "action", "id": action_id,
              "sequence": self.sequence, "session_id": self.session,
              "process_generation": self.generation, "target": self.target,
              "action": action}, allow_nan=False), flush=True)
        return action_id

    def start_motion(self):
        if self.motion:
            self.motion_id = self.send({"type": "motion.play", "motion_id": self.motion,
                                        "mode": "one_shot"})
            self.record("motion_pending")

    def start(self):
        self.started = True
        if self.expression:
            self.expression_id = self.send({"type": "expression.apply", "name": self.expression,
                                            "strength": 0.75, "duration_ms": 2000})
            self.record("expression_pending")
        else:
            self.start_motion()

    def run(self):
        self.record("waiting")
        packet = self.packet(self.deadline)
        require(packet is not None)
        self.session = packet["session_id"]
        self.generation = packet["process_generation"]
        handshake = packet["message"]
        require(handshake.get("type") == "handshake")
        settings = handshake.get("settings", {})
        grants = handshake.get("capabilities")
        require(isinstance(settings, dict) and set(settings) <= {
            "event", "character", "motion", "expression", "active"})
        require(isinstance(grants, list) and len(grants) <= 7
                and all(isinstance(g, str) and g in CAPABILITIES for g in grants)
                and len(set(grants)) == len(grants))
        selected = settings.get("event", "Reply finished")
        character = settings.get("character", "")
        self.motion = settings.get("motion", "studio.example.reaction-hook/nod")
        self.expression = settings.get("expression", "")
        active = settings.get("active", True)
        require(isinstance(selected, str) and selected in EVENTS
                and isinstance(character, str) and (not character or canonical_uuid(character))
                and type(active) is bool and text(self.motion, 193)
                and not any(c.isspace() for c in self.motion)
                and text(self.expression, 128)
                and (not self.expression or self.expression.strip())
                and not reserved(self.expression))
        if not active:
            self.record("ignored")
            return
        if not self.motion and not self.expression:
            self.record("completed")
            return
        require("lifecycle.observe" in grants
                and (not self.motion or "motion.play" in grants)
                and (not self.expression or "expression.apply" in grants))
        while True:
            now = time.monotonic()
            require(now < self.deadline)
            if (self.started and self.expression_id is None
                    and (not self.motion or (self.motion_accepted and self.motion_completed))
                    and (self.expression_until is None or now >= self.expression_until)):
                self.record("completed")
                return
            # While holding only the expression, still observe target invalidation.
            until = (self.expression_until if self.started and self.expression_id is None
                     and (not self.motion or (self.motion_accepted and self.motion_completed))
                     else self.deadline)
            packet = self.packet(until)
            if packet is None:
                continue
            message = packet["message"]
            if message.get("type") == "event":
                event = message.get("event")
                require(isinstance(event, dict) and isinstance(event.get("type"), str))
                kind = event["type"]
                if kind == "target.changed":
                    target = event.get("target")
                    require(target_ok(target))
                    if self.target is not None:
                        require(target == self.target)  # Never follow a replacement incarnation.
                    else:
                        self.target = dict(target)
                        if character and character != target["uuid"]:
                            self.record("ignored")
                            return
                elif kind in EVENTS.values() and not self.started and not self.testing:
                    require(self.source is None)  # One captured source per normal invocation.
                    self.source = event
                elif (kind == "motion.finished" and self.motion_id is not None
                      and event.get("action_id") == self.motion_id
                      and event.get("target") == self.target
                      and event.get("motion_id") == self.motion):
                    require(event.get("outcome") == "completed")
                    self.motion_completed = True
            elif message.get("type") == "result":
                action_id = message.get("id")
                if (message.get("target") == self.target and action_id is not None
                        and action_id in (self.expression_id, self.motion_id)):
                    result = message.get("result")
                    require(isinstance(result, dict) and result.get("status") == "ok"
                            and result.get("value") == {"type": "accepted"})
                    if action_id == self.expression_id:
                        self.expression_id = None
                        self.expression_until = time.monotonic() + 2.0
                        self.record("expression_accepted")
                        self.start_motion()
                    elif action_id == self.motion_id:
                        self.motion_accepted = True  # Acceptance alone cannot complete.
            else:
                raise Abort()
            if self.target is not None and not self.started:
                if self.testing:
                    self.start()
                elif self.source is not None:
                    source = self.source
                    if (source["type"] != EVENTS[selected]
                            or source.get("character") != self.target["uuid"]
                            or (source["type"] == "reply.finished"
                                and source.get("outcome") != "completed")):
                        self.record("ignored")
                        return
                    self.start()


if __name__ == "__main__":
    reaction = None
    try:
        require(sys.argv[1:] in ([], ["--test"]))
        reaction = Reaction(sys.argv[1:] == ["--test"])
        sys.exit(reaction.run())
    except (Abort, ValueError, KeyError, TypeError, OSError, UnicodeError, RecursionError):
        if reaction is not None:
            try:
                reaction.record("error")
            except OSError:
                pass
        print("Reaction hook stopped", file=sys.stderr)
        sys.exit(1)
