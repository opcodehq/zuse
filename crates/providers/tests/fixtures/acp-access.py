#!/usr/bin/env python3
"""Synthetic ACP peer: access-mode negotiation and permission ownership."""
import json
import sys

mode = "bypass"
profile = ""

def emit(frame):
    print(json.dumps(frame), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    if method == "initialize":
        emit({"id": frame["id"], "result": {"protocolVersion": 1, "agentCapabilities": {}}})
    elif method == "session/new":
        emit({"id": frame["id"], "result": {"sessionId": "access-parent", "modes": {
            "currentModeId": mode,
            "availableModes": [{"id": name, "name": name} for name in ["plan", "ask", "bypass"]]}}})
    elif method == "session/set_mode":
        mode = frame["params"]["modeId"]
        emit({"id": frame["id"], "result": {}})
    elif method == "session/prompt":
        profile = json.dumps(frame["params"]["prompt"])
        expected_mode = "plan" if "readonly" in profile else "ask" if "workspace" in profile or "full-ask" in profile else "bypass"
        if mode != expected_mode:
            emit({"id": frame["id"], "error": {"code": -32602, "message": "Incorrect access mode: " + mode}})
            break
        params = {"sessionId": "access-parent", "toolCall": {"toolCallId": "write", "title": "Write /tmp/access.txt", "kind": "edit"}, "options": [
            {"optionId": "once", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
            {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]}
        if "foreign" in profile:
            params["sessionId"] = "other-agent"
        if "missing-owner" in profile:
            del params["sessionId"]
        if "reject-only" in profile:
            params["options"] = [params["options"][2]]
        emit({"id": 990, "method": "session/request_permission", "params": params})
        answer = json.loads(sys.stdin.readline())
        outcome = answer.get("result", {}).get("outcome", {})
        cancelled = any(tag in profile for tag in ["readonly", "dropped", "foreign", "missing-owner", "reject-only"])
        wanted = {"outcome": "cancelled"} if cancelled else {"outcome": "selected", "optionId": "reject" if "deny" in profile else "once"}
        if outcome != wanted:
            emit({"id": frame["id"], "error": {"code": -32602, "message": "Incorrect permission response: " + json.dumps(outcome)}})
        else:
            emit({"id": frame["id"], "result": {"stopReason": "end_turn"}})
        break
