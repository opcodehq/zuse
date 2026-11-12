"""Exercise the installed launcher contract without provider credentials or compute."""
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time


def provider_keys():
    """Every ProviderId::api_key_env value, read from the Rust source of truth."""
    source = (Path(__file__).resolve().parents[3] / "crates/protocol/src/cloud_computers.rs").read_text()
    body = source[source.index("fn api_key_env"):]
    keys = re.findall(r'Self::\w+ => "([A-Z0-9_]+)"', body[:body.index("\n    }\n")])
    assert "PRIZED_API_KEY" in keys and len(keys) == len(set(keys)) >= 5, keys
    return keys


def wait_for(predicate):
    deadline = time.monotonic() + 10
    while not predicate():
        if time.monotonic() > deadline:
            raise AssertionError("recovery launcher did not reach expected state")
        time.sleep(0.05)


with tempfile.TemporaryDirectory(prefix="zuse-recovery-launcher-") as directory:
    root = Path(directory)
    launcher = root / "launch-rust-recovery"
    shutil.copyfile(Path(__file__).resolve().parents[1] / "launch-recovery.sh", launcher)
    launcher.chmod(0o700)
    helper = root / "helper"
    helper.write_text('''#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == recover && "$ZUSE_ARCHIVE_CONFIG" == '{}' ]]
[[ -z "${CLOUD_DATA_ENCRYPTION_KEY:-}" ]]
''' + "".join(f'[[ -z "${{{key}:-}}" ]]\n' for key in provider_keys()) + '''echo invoked >>"$TEST_ROOT/invocations"
if [[ "${TEST_LONG:-}" == yes ]]; then
  sleep 60 &
  echo "$!" >"$TEST_ROOT/child"
  wait
fi
if [[ "${TEST_RESTORE:-}" == yes ]]; then
  printf '%s\\n' workspace_archive_restored
else
  printf '%s\\n' workspace_archive_uploaded
fi
''')
    helper.chmod(0o700)
    env = dict(os.environ, ZUSE_RECOVERY_STATUS_ROOT=str(root), ZUSE_RUNTIME_BINARY=str(helper),
               TEST_ROOT=str(root), ZUSE_RUNTIME_PROVIDER="boat", ZUSE_RUNTIME_PERSISTENT_EXEC_CERTIFIED="1", CLOUD_DATA_ENCRYPTION_KEY="must-be-removed",
               **{key: "must-be-removed" for key in provider_keys()})

    def launch(operation, *args, **extra):
        subprocess.run([str(launcher), operation, *args], env=dict(env, **extra), check=True)

    def config(operation):
        state = root / operation
        state.mkdir()
        (state / "config.json").write_text("{}")
        return state

    state = config("complete")
    launch("complete")
    wait_for(lambda: (state / "uploaded").exists())
    launch("complete", "--foreground")
    assert (root / "invocations").read_text().splitlines() == ["invoked"]

    config("cancel-before-start")
    launch("cancel-before-start", "--cancel")
    launch("cancel-before-start", "--foreground")
    assert (root / "invocations").read_text().splitlines() == ["invoked"]

    state = config("cancel-running")
    launch("cancel-running", TEST_LONG="yes")
    wait_for(lambda: (root / "child").exists())
    child = int((root / "child").read_text())
    launch("cancel-running", "--cancel")
    # A killed child can briefly remain a zombie until the VM init reaps it.
    def stopped():
        status = Path(f"/proc/{child}/stat")
        return not status.exists() or status.read_text().split()[2] == "Z"
    wait_for(stopped)
    assert (state / "cancelled").exists() and not (state / "uploaded").exists()
    launch("cancel-running", "--foreground")
    assert len((root / "invocations").read_text().splitlines()) == 2
    state = config("restore")
    launch("restore", TEST_RESTORE="yes")
    wait_for(lambda: (state / "restored").exists())
    launch("restore", "--foreground", TEST_RESTORE="yes")
    assert len((root / "invocations").read_text().splitlines()) == 3
print("recovery launcher: upload, idempotency, pre-start cancellation and process-group cancellation and restore receipts passed")
