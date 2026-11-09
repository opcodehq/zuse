#!/usr/bin/env python3
"""Identical local CLI stimulus for Zuse and shipped T3; no provider network calls."""
import json
import os
import signal
import sys
import threading
import time
import uuid
import hashlib

SESSION = "8a2a521a-3c16-44a9-b719-8040658f28af"
cancelled = threading.Event()
lock = threading.Lock()
active = False


def log(event, **fields):
    with open(os.environ["ZUSE_MATCHED_PROVIDER_LOG"], "a") as file:
        file.write(json.dumps({"event": event, "at": time.time(), **fields}) + "\n")


def emit(value):
    with lock:
        print(json.dumps(value), flush=True)


def frame(event):
    emit({"type": "stream_event", "event": event, "session_id": SESSION,
          "uuid": str(uuid.uuid4()), "parent_tool_use_id": None})


def result(text, chunks, interrupted=False):
    emit({"type": "result", "subtype": "success", "result": text,
          "session_id": SESSION, "uuid": str(uuid.uuid4()), "is_error": False,
          "duration_ms": chunks * 50, "duration_api_ms": 0, "num_turns": 1,
          "total_cost_usd": 0, "stop_reason": "end_turn", "permission_denials": [],
          "usage": {"input_tokens": 1, "output_tokens": chunks,
                    "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0},
          "modelUsage": {}})


if "--version" in sys.argv:
    print("2.1.228 (Claude Code)")
    sys.exit(0)
if "auth" in sys.argv:
    emit({"loggedIn": True, "authMethod": "oauth", "email": "fixture@example.test"})
    sys.exit(0)


def respond(message):
    global active
    text = json.dumps(message, ensure_ascii=False)
    active = True
    cancelled.clear()
    log("run", message=message)
    emit({"type": "system", "subtype": "init", "cwd": os.getcwd(),
          "session_id": SESSION, "uuid": str(uuid.uuid4()), "tools": [],
          "mcp_servers": [], "model": "claude-sonnet-4-6", "permissionMode": "default",
          "slash_commands": [], "apiKeySource": "none", "claude_code_version": "2.1.228",
          "output_styles": [], "skills": [], "plugins": []})
    # The SDK's replay-user-messages handshake proves the active prompt was
    # accepted. Without its echo, T3 correctly holds unconfirmed output.
    echo = dict(message)
    echo.update(session_id=SESSION, parent_tool_use_id=None, isReplay=False)
    emit(echo)
    if "BENCHMARK tools" in text:
        # Synthetic native results; the fixture never executes these commands.
        # Both adapters see the same bulky values and may project them for UI.
        for number in range(3):
            tool_id = "fixture-tool-" + str(number) + "-" + uuid.uuid4().hex
            alphabet = "0123456789abcdef"
            output = "FIXTURE_FULL_OUTPUT_BEGIN\n" + (alphabet[number:] + alphabet[:number]) * 2048
            log("tool-output", tool_id=tool_id, bytes=len(output.encode()), sha256=hashlib.sha256(output.encode()).hexdigest())
            emit({"type": "assistant", "session_id": SESSION, "uuid": str(uuid.uuid4()),
                  "parent_tool_use_id": None, "message": {"id": "msg_" + uuid.uuid4().hex,
                  "type": "message", "role": "assistant", "model": "claude-sonnet-4-6",
                  "content": [{"type": "tool_use", "id": tool_id, "name": "Bash",
                               "input": {"command": "fixture-output-" + str(number)}}],
                  "stop_reason": "tool_use", "stop_sequence": None,
                  "usage": {"input_tokens": 1, "output_tokens": 1}}})
            emit({"type": "user", "session_id": SESSION, "uuid": str(uuid.uuid4()),
                  "parent_tool_use_id": None, "message": {"role": "user", "content": [
                  {"type": "tool_result", "tool_use_id": tool_id, "content": output,
                   "is_error": False}]}})
    chunks = 600 if "BENCHMARK stop" in text else 40
    reply = ""
    block = ""
    for index in range(chunks):
        if index % 5 == 0:
            block = ""
            message_id = "msg_" + uuid.uuid4().hex
            frame({"type": "message_start", "message": {"id": message_id, "type": "message",
                   "role": "assistant", "model": "claude-sonnet-4-6", "content": [],
                   "stop_reason": None, "stop_sequence": None,
                   "usage": {"input_tokens": 1, "output_tokens": 0}}})
            frame({"type": "content_block_start", "index": 0,
                   "content_block": {"type": "text", "text": ""}})
        if cancelled.wait(0.05):
            active = False
            return
        delta = f"Network packet {index:04d}. " + ("\n\n" if index % 5 == 4 else "")
        reply += delta
        block += delta
        frame({"type": "content_block_delta", "index": 0,
               "delta": {"type": "text_delta", "text": delta}})
        if index % 5 == 4:
            frame({"type": "content_block_stop", "index": 0})
            frame({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                   "usage": {"output_tokens": 5}})
            frame({"type": "message_stop"})
            emit({"type": "assistant", "session_id": SESSION, "uuid": str(uuid.uuid4()),
                  "parent_tool_use_id": None, "message": {"id": message_id, "type": "message",
                  "role": "assistant", "model": "claude-sonnet-4-6", "content": [{"type": "text", "text": block}],
                  "stop_reason": "end_turn", "stop_sequence": None,
                  "usage": {"input_tokens": 1, "output_tokens": 5}}})
    result(reply, chunks)
    active = False


def terminate(signum, _frame):
    if active:
        log("interrupt", signal=signum)
    cancelled.set()
    sys.exit(0)


signal.signal(signal.SIGTERM, terminate)
signal.signal(signal.SIGINT, terminate)
for line in sys.stdin:
    message = json.loads(line)
    if message.get("type") == "control_request":
        request = message.get("request", {})
        log("control", subtype=request.get("subtype"))
        if request.get("subtype") == "interrupt":
            log("interrupt", control=True)
            cancelled.set()
            active = False
        emit({"type": "control_response", "response": {"subtype": "success",
              "request_id": message["request_id"], "response": {
              "models": [{"value": "claude-sonnet-4-6", "displayName": "Sonnet 4.6", "description": "Local fixture"}],
              "commands": [], "agents": [], "account": {"email": "fixture@example.test"},
              "added": [], "removed": [], "errors": {}, "mcp_servers": []}}})
        if request.get("subtype") == "interrupt":
            emit({"type": "result", "subtype": "error_during_execution", "is_error": True,
                  "session_id": SESSION, "uuid": str(uuid.uuid4()), "errors": ["interrupted"],
                  "duration_ms": 0, "duration_api_ms": 0, "num_turns": 1, "total_cost_usd": 0,
                  "usage": {"input_tokens": 0, "output_tokens": 0}, "permission_denials": []})
    elif message.get("type") == "user":
        threading.Thread(target=respond, args=(message,), daemon=True).start()
