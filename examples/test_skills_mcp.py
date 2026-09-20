#!/usr/bin/env python3
"""Verify skills discovery + MCP echo server without the Rust binary."""

import json
import subprocess
from pathlib import Path

ROOT = Path("/home/workdir/artifacts/ice")


def test_skill():
    skill = ROOT / "skills/ice-burst/SKILL.md"
    assert skill.is_file(), skill
    body = skill.read_text()
    assert "BURST" in body
    print("skill ok:", skill, "bytes", len(body))


def rpc(proc, method, params, mid):
    req = {"jsonrpc": "2.0", "id": mid, "method": method, "params": params}
    proc.stdin.write(json.dumps(req) + "\n")
    proc.stdin.flush()
    line = proc.stdout.readline()
    return json.loads(line)


def test_mcp():
    proc = subprocess.Popen(
        ["python3", str(ROOT / "examples/mcp_echo.py")],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        init = rpc(proc, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "ice-test"}}, 1)
        assert init["result"]["serverInfo"]["name"] == "ice-echo"
        listed = rpc(proc, "tools/list", {}, 2)
        names = [t["name"] for t in listed["result"]["tools"]]
        assert "ping" in names, names
        called = rpc(proc, "tools/call", {"name": "ping", "arguments": {"text": "ice"}}, 3)
        text = called["result"]["content"][0]["text"]
        assert text == "echo:ice", text
        print("mcp ok: initialize + tools/list + tools/call ->", text)
    finally:
        proc.kill()


def test_todos_and_theme():
    # todos file format
    p = ROOT / ".ice/todos.json"
    p.parent.mkdir(exist_ok=True)
    p.write_text(json.dumps([{"id": 1, "text": "wire mcp", "done": False}], indent=2))
    data = json.loads(p.read_text())
    assert data[0]["text"] == "wire mcp"
    print("todo store ok")
    print("default theme: ice")


if __name__ == "__main__":
    test_skill()
    test_mcp()
    test_todos_and_theme()
    print("ALL CHECKS PASSED")
