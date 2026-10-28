"""Local Waifu Studio control client / stdio MCP server (Python stdlib only).

Start the app with --control-dir DIRECTORY, then use:
  py tools/studio_control.py --control-dir DIRECTORY --json '{"action":"status"}'
  py tools/studio_control.py --control-dir DIRECTORY --mcp
No mouse injection, sockets, arbitrary execution or access to app credentials.
"""
import argparse
import json
import os
from pathlib import Path
import sys
import time
import uuid


def send(directory, command, timeout=30):
    session = json.loads((directory / "session.json").read_text(encoding="utf-8"))
    request_id = str(uuid.uuid4())
    request = {"id": request_id, "session": session["session"], "command": command}
    encoded = json.dumps(request).encode("utf-8")
    if len(encoded) > 65536:
        raise ValueError("Request exceeds 64 KiB")
    pending = directory / "request.json"
    if pending.exists():
        raise RuntimeError("Another request is pending")
    temp = directory / (request_id + ".tmp")
    temp.write_bytes(encoded)
    os.replace(temp, pending)
    response = directory / ("response-" + request_id + ".json")
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            value = json.loads(response.read_text(encoding="utf-8"))
        except (FileNotFoundError, json.JSONDecodeError):
            time.sleep(0.05)
            continue
        response.unlink(missing_ok=True)
        if value.get("id") != request_id:
            raise RuntimeError("Response ID mismatch")
        if not value.get("ok"):
            raise RuntimeError(value.get("error", "App rejected request"))
        return value["result"]
    raise TimeoutError("App did not respond; verify it is running with this control directory")


def serve(directory):
    """MCP stdio JSON-RPC transport; stdout contains protocol messages only."""
    tools = [{
        "name": "waifu_control",
        "description": "Inspect/control an explicitly opted-in local Waifu Studio instance. Actions: status, list_motions, play_motion (catalog path), stop_motion, set_gaze (config), set_view (studio/desktop), set_screen (screen), sit (sitting boolean), capture (plain PNG name). No global mouse/keyboard.",
        "inputSchema": {"type": "object", "properties": {"action": {"type": "string", "enum": ["status", "list_motions", "play_motion", "stop_motion", "set_automatic_motion", "set_frame_caps", "set_camera_view", "restore_window", "set_gaze", "set_view", "set_screen", "sit", "capture"]}, "path": {"type": "string"}, "config": {"type": "object"}, "view": {"type": "string"}, "screen": {"type": "string"}, "sitting": {"type": "boolean"}, "name": {"type": "string"}, "enabled": {"type": "boolean"}, "focused": {"type": "integer"}, "background": {"type": "integer"}, "desktop": {"type": "integer"}}, "required": ["action"], "additionalProperties": False},
    }]
    for line in sys.stdin:
        try:
            message = json.loads(line)
            if "id" not in message:
                continue
            method = message.get("method")
            if method == "initialize":
                result = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}}, "serverInfo": {"name": "waifu-studio-local-control", "version": "1.0"}}
            elif method == "ping":
                result = {}
            elif method == "tools/list":
                result = {"tools": tools}
            elif method == "tools/call":
                params = message.get("params", {})
                if params.get("name") != "waifu_control":
                    raise ValueError("Unknown tool")
                try:
                    value = send(directory, params.get("arguments", {}))
                    result = {"content": [{"type": "text", "text": json.dumps(value)}]}
                except Exception as error:
                    result = {"isError": True, "content": [{"type": "text", "text": str(error)}]}
            else:
                raise ValueError("Unknown method")
            response = {"jsonrpc": "2.0", "id": message["id"], "result": result}
        except Exception as error:
            response = {"jsonrpc": "2.0", "id": None, "error": {"code": -32600, "message": str(error)}}
        print(json.dumps(response), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--control-dir", required=True, type=Path)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--json")
    group.add_argument("--mcp", action="store_true")
    args = parser.parse_args()
    if args.mcp:
        serve(args.control_dir)
    else:
        print(json.dumps(send(args.control_dir, json.loads(args.json)), indent=2))
