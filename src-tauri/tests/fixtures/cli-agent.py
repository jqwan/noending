#!/usr/bin/env python3
"""Protocol stub for the Unix standalone CLI extraction integration test.
Requires Python 3; it reads every input through the actual scoped CLI connection.
"""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time

if sys.argv[1:] == ["--version"]:
    print("codex-cli protocol-test")
    sys.exit(0)

prompt = sys.argv[-1]
prefix = next(line.removeprefix("CLI 前缀：") for line in prompt.splitlines() if line.startswith("CLI 前缀："))
argv = shlex.split(prefix)
Path(os.environ["NE_TEST_READY"]).write_text(str(os.getpid()))
release = Path(os.environ["NE_TEST_RELEASE"])
started = time.monotonic()
while not release.exists():
    if time.monotonic() - started > 15:
        sys.exit("test did not release the extraction")
    time.sleep(0.02)

def query(*args):
    value = json.loads(subprocess.check_output([*argv, *args], text=True))
    assert value["ok"], value
    return value["data"]

plan = query("inputs")
for entry in plan["inputs"]:
    name = entry["name"]
    cursor = 0
    while True:
        page = query("read", "--input", name, "--cursor", str(cursor), "--limit", "8")
        cursor = page.get("next_cursor")
        if cursor is None:
            break
key = plan["result_key"]
payload = Path("submission.json")
payload.write_text(json.dumps({
    "reply": {"status": "completed"},
    "results": {key: {
        "summary": {"items": [{"title": "Confirmed memory", "description": "Saved before exit", "details": ["Read through the scoped CLI"]}]}
    }}
}))
receipt = query("submit", "--file", str(payload))
assert receipt["status"] == "updated", receipt
print("Saved through NoEnding")
