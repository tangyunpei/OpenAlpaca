#!/usr/bin/env python3
"""Stub plugin for testing: answers a tool call only by the name it listed.

Speaks JSON-RPC 2.0 over Content-Length-framed stdio (LSP/MCP standard),
as tests/fixtures/echo-plugin/echo-server.sh does, but in one process and
strict about the tool's name. Responds to:
  tools/list  -> one tool, "echo", with a "message" string and an optional
                 "fail" boolean
  tools/call  -> for params.name == "echo": "echo: <message>", or, when
                 "fail" is true, a JSON-RPC error; for any other name a
                 JSON-RPC error naming the name it was asked for
  everything else (a request) -> an empty result; a notification -> nothing

It leads a process group of its own, so a test can end it, and anything it
might have started, with one signal to the group.
"""

import json
import os
import sys

LISTED = "echo"

os.setpgid(0, 0)

stdin = sys.stdin.buffer
stdout = sys.stdout.buffer


def read_message():
    length = None
    while True:
        line = stdin.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            if length is not None:
                break
            continue
        name, _, value = line.partition(b":")
        if name.strip().lower() == b"content-length":
            length = int(value.strip())
    return json.loads(stdin.read(length))


def send(message):
    body = json.dumps(message).encode()
    stdout.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    stdout.flush()


def error(mid, message):
    return {"jsonrpc": "2.0", "id": mid, "error": {"code": -32602, "message": message}}


def answer(msg):
    mid = msg["id"]
    method = msg.get("method", "")
    params = msg.get("params") or {}
    if method == "tools/list":
        schema = {
            "type": "object",
            "properties": {"message": {"type": "string"}, "fail": {"type": "boolean"}},
            "required": ["message"],
        }
        tool = {"name": LISTED, "description": "Echo back the input", "inputSchema": schema}
        return {"jsonrpc": "2.0", "id": mid, "result": {"tools": [tool]}}
    if method == "tools/call":
        name = params.get("name")
        if name != LISTED:
            return error(mid, "no tool named %r; this plugin lists %r" % (name, LISTED))
        args = params.get("arguments") or {}
        if args.get("fail"):
            return error(mid, "echo was asked to fail")
        text = "echo: %s" % args.get("message", "no message")
        return {"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": text}]}}
    return {"jsonrpc": "2.0", "id": mid, "result": {}}


while True:
    msg = read_message()
    if msg is None:
        sys.exit(0)
    if "id" in msg:
        send(answer(msg))
