#!/usr/bin/env python3
"""Exercise the complete evaluation runner against a deterministic local model.

This verifies transport/tool/journal integration and caps, not model quality,
provider caching or subscription savings. It sends no external model requests.
"""
import argparse
import hashlib
import importlib.util
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import subprocess
import tempfile
import threading

module_spec = importlib.util.spec_from_file_location("evaluation", Path(__file__).with_name("evaluate-harness.py"))
evaluation = importlib.util.module_from_spec(module_spec)
module_spec.loader.exec_module(evaluation)


def replies(body):
    history = body["input"]
    prompt = next((i["content"][0]["text"] for i in history if i.get("role") == "user"), "")
    if prompt.startswith("Reply ready"):
        return [], "ready"
    calls = [i for i in history if i.get("type") == "function_call"]
    results = {i["call_id"]: json.loads(i["output"]) for i in history if i.get("type") == "function_call_output"}
    if prompt == "Exercise the request cap":
        return [(f"cap-{len(calls)}", "list_directory", {"path": "."})], ""
    if "clamp" in prompt:
        files = {"ranges.py": "def clamp(value, low, high):\n    return min(high, max(value, low))\n"}
        case = evaluation.cases()[0]
    elif "format_count" in prompt:
        files = {"counts.py": "def format_count(n):\n    return str(n) + (' item' if n == 1 else ' items')\n\nlabel = format_count\n",
                 "report.py": "from counts import format_count\ndef report(values):\n    return format_count(len(values))\n"}
        case = evaluation.cases()[1]
    elif "parse_level" in prompt:
        files = {"parse.py": "def parse_level(line):\n    return line.split(':')[0].strip().lower()\n"}
        case = evaluation.cases()[2]
    else:
        files = {"parser.py": "def enabled(value):\n    if isinstance(value, bool):\n        return value\n    if isinstance(value, str) and value.strip().lower() in ('true', 'false'):\n        return value.strip().lower() == 'true'\n    raise ValueError(value)\n"}
        case = evaluation.cases()[3]
    if not calls:
        reads = [(f"read-{index}", "read_file", {"path": path}) for index, path in enumerate(files)]
        if case["id"] == "noisy-evidence":
            reads.append(("read-log", "read_file", {"path": "application.log", "limit": 2000}))
        return reads, ""
    if case["id"] == "noisy-evidence" and "read-artifact" not in results:
        return [("read-artifact", "read_artifact", {"artifact": results["read-log"]["artifact"], "max_bytes": 64000})], ""
    if not any(i["call_id"].startswith("write-") for i in calls):
        return [(f"write-{index}", "write_file", {"path": path, "content": content, "expected_hash": results[f"read-{index}"]["hash"]}) for index, (path, content) in enumerate(files.items())], ""
    if "verify" not in results:
        import shlex
        return [("verify", "exec_command", {"command": "python3 -c " + shlex.quote(case["test"]), "yield_time_ms": 1000, "timeout_ms": 10000})], ""
    return [], "Completed and verified fixture task"


class Model(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    requests = 0
    counter_lock = threading.Lock()

    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        with self.counter_lock:
            type(self).requests += 1
        if body["model"] == "fixture-rate-limited":
            encoded = json.dumps({"error": {"type": "rate_limit_error", "message": "Fixture rate limit"}}).encode()
            self.send_response(429)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)
            self.wfile.flush()
            return
        calls, text = replies(body)
        items = [{"type": "function_call", "call_id": cid, "name": name, "arguments": json.dumps(arguments)} for cid, name, arguments in calls]
        if text:
            items.append({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]})
        events = []
        if text:
            events.append({"type": "response.output_text.delta", "delta": text})
        events.extend({"type": "response.output_item.done", "output_index": index, "item": item} for index, item in enumerate(items))
        events.append({"type": "response.completed", "response": {"status": "completed"}})
        encoded = "".join("data: " + json.dumps(event) + "\n\n" for event in events).encode()
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


class Server(ThreadingHTTPServer):
    connections = 0
    def get_request(self):
        result = super().get_request()
        self.connections += 1
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=False)
    with Server(("127.0.0.1", 0), Model) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            connection = args.output / "fixture-connection.json"
            connection.write_text(json.dumps({"connectionId": "local-fixture", "accessToken": "local-fixture-not-a-provider-token",
                "model": "fixture", "reasoning": None, "contextWindowTokens": 1000000,
                "route": {"endpoint": f"http://127.0.0.1:{server.server_port}/responses", "protocol": "responses", "subscription": False,
                          "capabilities": {"images": True, "tools": True, "reasoning": True, "promptCache": True}, "maxOutputTokens": 4096}}))
            connection.chmod(0o600)
            manifest = args.output / "manifest.json"
            manifest.write_text(json.dumps({"arms": [{"label": "native-fixture", "protocol": "native-evaluation-v1", "command": [str(binary)], "connection": str(connection.resolve()), "simulation": True}]}))
            completed = subprocess.run(["python3", str(Path(__file__).with_name("evaluate-harness.py")), "--manifest", str(manifest), "--output", str(args.output / "runs"), "--global-ledger", str(args.output.resolve() / "fixture-global.ledger")], timeout=300)
            report_path = args.output / "runs/report.json"
            report = json.loads(report_path.read_text())
            assert completed.returncode == 0, f"See {report_path}"
            assert len(report["runs"]) == 15
            assert all(run["status"] == "passed" for run in report["runs"]), f"See {report_path}"
            assert report["reserved_requests"] == Model.requests <= 300
            assert all(run["peak_tree_rss_bytes"] > 0 and run["peak_tree_pss_bytes"] > 0 for run in report["runs"])
            assert all(run["cpu_seconds"] >= 0 and run["process_output_block_bytes"] >= 0 for run in report["runs"])
            assert any(run["peak_processes"] > 1 for run in report["runs"]), "Command helpers must count toward process-tree memory"

            # Force an endless tool loop and prove both root and physical caps.
            cap = args.output / "cap-test"
            cap.mkdir()
            (cap / "checkout").mkdir()
            (cap / "task.txt").write_text("Exercise the request cap")
            before = Model.requests
            command = [str(binary), "--checkout", str(cap / "checkout"), "--profile", str(cap / "profile"), "--connection", str(connection.resolve()), "--task", str(cap / "task.txt"), "--global-ledger", str(cap / "global"), "--run-ledger", str(cap / "local")]
            capped = subprocess.run(command, capture_output=True, text=True, timeout=30)
            assert capped.returncode != 0
            assert Model.requests - before == 6
            outcomes = [json.loads(line) for line in capped.stdout.splitlines()]
            assert outcomes[-1]["status"] == "capped"
            (cap / "events.jsonl").write_text(capped.stdout)
            # A provider failure on the final allowed request is not a cap.
            rate = args.output / "rate-limit-test"
            rate.mkdir()
            (rate / "checkout").mkdir()
            (rate / "task.txt").write_text("Reply ready")
            config = json.loads(connection.read_text())
            config["model"] = "fixture-rate-limited"
            rate_connection = rate / "connection.json"
            rate_connection.write_text(json.dumps(config))
            rate_connection.chmod(0o600)
            before = Model.requests
            failed = subprocess.run([str(binary), "--checkout", str(rate / "checkout"), "--profile", str(rate / "profile"),
                "--connection", str(rate_connection), "--task", str(rate / "task.txt"), "--global-ledger", str(rate / "global"),
                "--run-ledger", str(rate / "local"), "--request-limit", "1"], capture_output=True, text=True, timeout=30)
            assert failed.returncode != 0 and Model.requests - before == 1
            outcomes = [json.loads(line) for line in failed.stdout.splitlines()]
            assert outcomes[-1]["status"] == "failed"
            assert outcomes[0]["attempt"]["errorCode"] == "http_429"
            (rate / "events.jsonl").write_text(failed.stdout)
            assert server.connections < Model.requests, "Native HTTP connections were not reused"
            summary = {"mode": "scripted-local-http", "coding_runs": 12, "smoke_runs": 3, "capped_run": 1, "rate_limited_run": 1,
                       "external_model_requests": 0, "local_http_requests": Model.requests, "tcp_connections": server.connections,
                       "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "status": "passed"}
            (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
            print(json.dumps(summary))
        finally:
            server.shutdown()
            thread.join()


if __name__ == "__main__":
    main()
