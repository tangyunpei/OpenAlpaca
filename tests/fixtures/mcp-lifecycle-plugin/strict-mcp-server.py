#!/usr/bin/env python3
"""Stub plugin for testing: an MCP server strict about its lifecycle.

Speaks JSON-RPC 2.0 over Content-Length-framed stdio, as the plugin channel
does (tests/fixtures/named-plugin/named-server.py), and refuses what an rmcp
server refuses before `initialize` (rmcp-3.9.0 `src/service/server.rs`,
`serve_server_with_ct_inner`, as locked):
  a request before `initialize`      -> a JSON-RPC error (-32602, the
                                        request's `_meta` lacks what the
                                        2026-07-28 lifecycle requires), and it
                                        waits on for `initialize`
  a notification before `initialize` -> it exits (rmcp's
                                        `ExpectedInitializeRequest`)
  `initialize` without `protocolVersion`, `capabilities` and `clientInfo`
                                     -> a JSON-RPC error (-32602)
  `initialize` while STRICT_STUB_INIT_ERROR is set in its environment
                                     -> a JSON-RPC error (-32603), and it
                                        waits on, still uninitialized (a
                                        server that refuses the handshake)
Once initialized it answers:
  tools/list  -> one tool, "echo", with a "message" string
  tools/call  -> "echo: <message>"
  any other request -> -32601 (method not found), as an MCP server does
  a notification -> nothing
Every message's method is appended to ./seen.log and the `initialize`
params are written to ./initialize-params.json (the child's cwd is its
plugin directory), so a test can read what it was sent and in what order.

It leads a process group of its own, so a test can end it, and anything it
might have started, with one signal to the group.
"""

import json
import os
import sys

os.setpgid(0, 0)

stdin = sys.stdin.buffer
stdout = sys.stdout.buffer

# The versions rmcp-3.9.0 serves that have an `initialize` handshake.
KNOWN = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]


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


def error(mid, code, message):
    return {"jsonrpc": "2.0", "id": mid, "error": {"code": code, "message": message}}


def record(msg):
    with open("seen.log", "a") as log:
        log.write(msg.get("method", "<response>") + "\n")


def initialize(msg):
    mid = msg["id"]
    params = msg.get("params") or {}
    with open("initialize-params.json", "w") as f:
        json.dump(params, f)
    if os.environ.get("STRICT_STUB_INIT_ERROR"):
        return error(mid, -32603, "this server refuses the handshake")
    if not (
        isinstance(params.get("protocolVersion"), str)
        and isinstance(params.get("capabilities"), dict)
        and isinstance(params.get("clientInfo"), dict)
    ):
        return error(mid, -32602, "initialize needs protocolVersion, capabilities and clientInfo")
    requested = params["protocolVersion"]
    version = requested if requested in KNOWN else KNOWN[-1]
    result = {
        "protocolVersion": version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "strict-mcp", "version": "0.0.1"},
    }
    return {"jsonrpc": "2.0", "id": mid, "result": result}


def answer(msg):
    mid = msg["id"]
    method = msg.get("method", "")
    params = msg.get("params") or {}
    if method == "tools/list":
        schema = {
            "type": "object",
            "properties": {"message": {"type": "string"}},
            "required": ["message"],
        }
        tool = {"name": "echo", "description": "Echo back the input", "inputSchema": schema}
        return {"jsonrpc": "2.0", "id": mid, "result": {"tools": [tool]}}
    if method == "tools/call":
        args = params.get("arguments") or {}
        text = "echo: %s" % args.get("message", "no message")
        return {"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": text}]}}
    return error(mid, -32601, "method not found: %s" % method)


initialized = False
while True:
    msg = read_message()
    if msg is None:
        sys.exit(0)
    record(msg)
    if not initialized:
        if "id" not in msg:
            sys.exit(1)
        if msg.get("method") != "initialize":
            send(error(msg["id"], -32602, "request _meta is missing or has malformed required fields"))
            continue
        reply = initialize(msg)
        initialized = "result" in reply
        send(reply)
        continue
    if "id" in msg:
        send(answer(msg))
