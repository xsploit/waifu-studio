#!/usr/bin/env python3
"""Owned offline event replay. No network, secrets, chat model, or retry loop."""
import json
import os
from pathlib import Path
import sys


def describe(packet):
    event = packet["event"]
    kind = packet["subscription"]["type"]
    if kind == "channel.subscribe":
        return f"[Synthetic Twitch subscription] {event['user_name']} subscribed at tier {event['tier']}."
    if kind == "channel.cheer":
        return f"[Synthetic Twitch cheer] {event['user_name']} cheered {event['bits']} bits: {event['message']}"
    if kind == "channel.chat.message":
        return f"[Synthetic Twitch chat] {event['chatter_user_name']}: {event['message']['text']}"
    raise ValueError("Unsupported replay event")


def run():
    handshake = json.loads(sys.stdin.readline())
    fixtures = json.loads(Path(__file__).with_name("fixtures.json").read_text())
    choice = handshake["message"].get("settings", {}).get("event", "Subscription")
    text = describe(fixtures[choice])
    receipt = Path(os.environ["WAIFU_EXTENSION_DATA"]) / "latest-replay.json"
    sequence = 0
    sent = False
    nod_sent = False
    target = None
    cancelled = False

    def record(value):
        receipt.write_text(json.dumps(value))

    def action(action_id, body):
        nonlocal sequence
        sequence += 1
        print(json.dumps({
            "api_version": 1, "type": "action", "id": action_id,
            "session_id": handshake["session_id"],
            "process_generation": handshake["process_generation"],
            "sequence": sequence, "target": target, "action": body,
        }), flush=True)

    record({"state": "waiting_for_target", "example": choice})
    for line in sys.stdin:
        envelope = json.loads(line)
        if envelope["session_id"] != handshake["session_id"]:
            raise ValueError("Session changed")
        message = envelope["message"]
        if message["type"] == "result":
            record(message)
            if message["id"] == "replay-message" and message["result"]["status"] != "error" and not cancelled and not nod_sent:
                nod_sent = True
                action("replay-nod", {"type": "motion.play", "motion_id": "studio.example.twitch-replay/nod", "mode": "one_shot"})
            continue
        if message["type"] != "event":
            continue
        event = message["event"]
        if event["type"] == "target.changed":
            if sent and event["target"] != target:
                cancelled = True
                record({"state": "cancelled", "reason": "Original target changed"})
                continue
            target = event["target"]
            if target and not sent:
                sent = True
                action("replay-message", {"type": "message.deliver", "text": text})
        elif event["type"] == "motion.finished" and event["action_id"] == "replay-nod":
            record({"state": "finished", "example": choice, "motion": event})
        # Stay connected after the single replay; Restart explicitly replays.


if __name__ == "__main__":
    run()
