#!/usr/bin/env bash
# Prefer a separate Ubuntu mirror if the runner's Azure mirror has DNS issues.
set -euo pipefail

source_dir=$(mktemp -d)
trap 'rm -rf "$source_dir"' EXIT
chmod 755 "$source_dir"
cat > "$source_dir/ubuntu.list" <<'SOURCES'
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://mirrors.edge.kernel.org/ubuntu noble main restricted universe multiverse
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://mirrors.edge.kernel.org/ubuntu noble-updates main restricted universe multiverse
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://mirrors.edge.kernel.org/ubuntu noble-security main restricted universe multiverse
SOURCES
options=(-o "Dir::Etc::sourcelist=$source_dir/ubuntu.list" -o Dir::Etc::sourceparts=- -o Acquire::Retries=1 -o Acquire::http::Timeout=15 -o Acquire::https::Timeout=15)
if sudo apt-get "${options[@]}" -o APT::Update::Error-Mode=any update && \
   sudo apt-get "${options[@]}" install -y "$@"; then
  exit 0
fi
printf '%s\n' 'Alternative Ubuntu mirror unavailable; retrying the runner sources.' >&2
sudo apt-get -o Acquire::Retries=1 -o Acquire::http::Timeout=15 -o Acquire::https::Timeout=15 -o APT::Update::Error-Mode=any update
sudo apt-get -o Acquire::Retries=1 -o Acquire::http::Timeout=15 -o Acquire::https::Timeout=15 install -y "$@"
