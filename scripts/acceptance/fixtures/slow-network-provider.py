#!/usr/bin/env python3
"""Local streaming provider for network acceptance; never calls a provider API."""
import json
import os
import sys
import threading
import time


def emit(value):
    print(json.dumps(value), flush=True)


if "--version" in sys.argv:
    print("2.1.228")
    sys.exit(0)
if "auth" in sys.argv:
    emit({"loggedIn": True, "authMethod": "oauth", "email": "network@example.test"})
    sys.exit(0)

cancelled = threading.Event()


def respond(message):
    with open(os.environ["ZUSE_IMPORT_PROVIDER_SEEN"], "a") as log:
        log.write(json.dumps({"event": "run", "message": message}) + "\n")
    text = json.dumps(message)
    chunks = 600 if "stop this response" in text else 40
    reply = ""
    for index in range(chunks):
        if cancelled.wait(0.05):
            return
        delta = f"Network packet {index:04d}. "
        reply += delta
        emit({"type": "stream_event", "event": {"type": "content_block_delta",
              "delta": {"type": "text_delta", "text": delta}}})
    emit({"type": "result", "subtype": "success", "result": reply,
          "usage": {"input_tokens": 1, "output_tokens": chunks},
          "session_id": "original-claude-session"})


for line in sys.stdin:
    message = json.loads(line)
    if message.get("type") == "control_request":
        request = message.get("request", {})
        if request.get("subtype") == "interrupt":
            cancelled.set()
            with open(os.environ["ZUSE_IMPORT_PROVIDER_SEEN"], "a") as log:
                log.write(json.dumps({"event": "interrupt"}) + "\n")
        emit({"type": "control_response", "response": {
            "subtype": "success", "request_id": message["request_id"],
            "response": {"models": [{"value": "claude-sonnet-4-6", "displayName": "Sonnet 4.6"}], "commands": []}}})
    elif message.get("type") == "user":
        if "--system-prompt" in sys.argv:
            emit({"type": "result", "subtype": "success", "result": "Network acceptance"})
        else:
            cancelled.clear()
            threading.Thread(target=respond, args=(message,), daemon=True).start()
