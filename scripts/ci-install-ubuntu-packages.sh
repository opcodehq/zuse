#!/usr/bin/env bash
# Use the runner's configured repositories first, then retry against Ubuntu's
# official repositories if its mirror list or package installation fails.
set -euo pipefail

if sudo apt-get -o Acquire::Retries=1 -o Acquire::http::Timeout=15 -o Acquire::https::Timeout=15 -o APT::Update::Error-Mode=any update && \
   sudo apt-get -o Acquire::Retries=1 -o Acquire::http::Timeout=15 -o Acquire::https::Timeout=15 install -y "$@"; then
  exit 0
fi

printf '%s\n' 'Configured Ubuntu mirrors failed; retrying with official repositories.' >&2
sources=$(mktemp)
trap 'rm -f "$sources"' EXIT
cat > "$sources" <<'SOURCES'
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://archive.ubuntu.com/ubuntu noble main restricted universe multiverse
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://archive.ubuntu.com/ubuntu noble-updates main restricted universe multiverse
deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://security.ubuntu.com/ubuntu noble-security main restricted universe multiverse
SOURCES
options=(-o "Dir::Etc::sourcelist=$sources" -o Dir::Etc::sourceparts=- -o Acquire::Retries=2 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30)
sudo apt-get "${options[@]}" -o APT::Update::Error-Mode=any update
sudo apt-get "${options[@]}" install -y "$@"
