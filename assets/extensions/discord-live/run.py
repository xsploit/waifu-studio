#!/usr/bin/env python3
"""Launch the native bot worker. Protocol stdout is reserved for the host."""
import os
import sys
host = os.environ.get("WAIFU_STUDIO_EXECUTABLE")
if not host:
    sys.exit("Launch this integration through Waifu Studio")
if os.name == "nt":
    # Windows execv creates a new process and exits this supervised leader.
    # Keep the leader alive until its native child finishes in the same Job.
    import subprocess
    sys.exit(subprocess.call([host, "--discord-extension"]))
os.execv(host, [host, "--discord-extension"])
