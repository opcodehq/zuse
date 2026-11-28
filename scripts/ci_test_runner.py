"""Give each nextest process tree its own temporary files and effect locks."""
import os
import subprocess
import sys
import tempfile


def run(arguments):
    with tempfile.TemporaryDirectory(prefix="ci-", dir="/tmp") as directory:
        environment = dict(os.environ, TMPDIR=directory, TMP=directory, TEMP=directory)
        return subprocess.call(arguments, env=environment)


if __name__ == "__main__":
    raise SystemExit(run(sys.argv[1:]))
