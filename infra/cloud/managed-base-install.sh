#!/usr/bin/env bash
# Runs only inside a fresh isolated provider builder, never on the API host.
set -euo pipefail
umask 077
bundle_root="${1:?bundle directory required}"
# Provider template builds can lay out files before their resident boot command
# exists. Layout is never readiness or certification; prepare still needs the
# live private Supervisor and all ordinary image/sanitation/native gates.
stage="${2:-all}"
case "$stage" in all|layout|prepare) ;; *) printf '%s\n' invalid_base_install_stage >&2; exit 2;; esac
[[ "$bundle_root" == /opt/zuse-base-input && "$(id -u)" == 0 ]]
if [[ "$stage" != prepare ]]; then
# An unchanged harness is reused only after the restored clean lineage, every
# installed native companion and absence of task state have been verified.
if [[ -n "${ZUSE_CLEAN_DERIVATION_SHA256:-}" ]]; then
  [[ "$ZUSE_CLEAN_DERIVATION_SHA256" =~ ^[a-f0-9]{64}$ ]]
  python3 -I -B "$bundle_root/managed-base-derive.py" --input "$bundle_root" --sha256 "$ZUSE_CLEAN_DERIVATION_SHA256" --operation-id "${ZUSE_CLEAN_DERIVATION_OPERATION_ID:?original builder operation required}"
elif [[ -e "$bundle_root/derivation.json" ]]; then
  printf '%s\n' derivation_digest_required >&2; exit 1
fi
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
# openssl (>= 3.0) verifies Ed25519 runtime release signatures in runtime-update.py.
apt-get install -y -qq ca-certificates git gh openssh-client ripgrep python3 build-essential sudo util-linux zlib1g openssl curl xz-utils
# Node.js LTS from the official nodejs.org build, pinned to one version and its
# SHA-256 from that release's signed SHASUMS256.txt. Distro nodejs is too old.
# 24.x is the newest LTS line that still bundles corepack. Unpacked straight
# into /usr/local so root's npm prefix is /usr/local: node, npm, npx and
# corepack are on every PATH, and `sudo npm i -g X` lands in /usr/local/bin.
node_version=24.22.3
node_sha256=4f2b175a6d53a5c617a80dba2676238a0688cef28c6da4eba35c7d844e66fcfb
if [[ "$(/usr/local/bin/node --version 2>/dev/null || true)" != "v$node_version" ]]; then
  [[ "$(uname -m)" == x86_64 ]] || { printf '%s\n' node_architecture_unsupported >&2; exit 1; }
  node_work="$(mktemp -d /var/tmp/zuse-node.XXXXXX)"
  curl -fsSL --retry 3 --proto '=https' -o "$node_work/node.tar.xz" \
    "https://nodejs.org/dist/v$node_version/node-v$node_version-linux-x64.tar.xz"
  printf '%s  %s\n' "$node_sha256" "$node_work/node.tar.xz" | sha256sum -c --quiet - \
    || { printf '%s\n' node_checksum_mismatch >&2; exit 1; }
  tar -xJf "$node_work/node.tar.xz" -C "$node_work" --no-same-owner
  node_tree="$node_work/node-v$node_version-linux-x64"
  chown -R -h root:root "$node_tree"
  chmod -R u+rwX,go+rX,go-w "$node_tree"
  cp -a "$node_tree/bin/." /usr/local/bin/
  for part in include lib share; do
    mkdir -p "/usr/local/$part"
    cp -a "$node_tree/$part/." "/usr/local/$part/"
  done
  rm -rf -- "$node_work"
fi
[[ "$(/usr/local/bin/node --version)" == "v$node_version" ]]
# The npm/npx/corepack entries are `#!/usr/bin/env node` scripts; probe them
# with a throwaway HOME so no npm cache or log lands in /root.
node_probe_home="$(mktemp -d /var/tmp/zuse-node-probe.XXXXXX)"
for tool in npm npx corepack; do
  env -i HOME="$node_probe_home" PATH=/usr/local/bin:/usr/bin:/bin "/usr/local/bin/$tool" --version >/dev/null \
    || { printf '%s\n' "node_tool_missing_$tool" >&2; exit 1; }
done
rm -rf -- "$node_probe_home"
# SSH access starts its own loopback-only sshd on demand. Images without an SSH
# server (Daytona's container base) get the binary, never a listening service,
# and no shared host keys: each computer generates its own on first use.
if [[ ! -x /usr/sbin/sshd ]]; then
  created_policy=0
  if [[ ! -e /usr/sbin/policy-rc.d ]]; then
    printf '#!/bin/sh\nexit 101\n' > /usr/sbin/policy-rc.d
    chmod 755 /usr/sbin/policy-rc.d
    created_policy=1
  fi
  apt-get install -y -qq --no-install-recommends openssh-server
  if [[ "$created_policy" == 1 ]]; then rm -f /usr/sbin/policy-rc.d; fi
  if command -v systemctl >/dev/null 2>&1; then
    systemctl disable ssh.service ssh.socket >/dev/null 2>&1 || true
  fi
  rm -f /etc/ssh/ssh_host_*_key /etc/ssh/ssh_host_*_key.pub
  [[ -x /usr/sbin/sshd ]]
fi
# Authenticate gh at command time through the task broker; never gh auth login.
[[ -f /usr/bin/gh && ! -L /usr/bin/gh ]]
/usr/bin/gh --version | head -n 1 | grep -E '^gh version 2\.' >/dev/null
apt-get install -y -qq libssl3t64 || apt-get install -y -qq libssl3
apt-get install -y -qq libasound2t64 || apt-get install -y -qq libasound2
getent group zuse >/dev/null || groupadd --gid 10001 zuse
id zuse >/dev/null 2>&1 || useradd --uid 10001 --gid zuse --shell /bin/bash --create-home zuse
[[ "$(id -u zuse)" == 10001 && "$(getent passwd zuse | cut -d: -f6)" == /home/zuse ]]
install -m 755 "$bundle_root/runtime" /usr/local/bin/zuse-cloud-runtime
if [[ -L /usr/local/bin/zuse-grok-auth ]]; then
  [[ "$(readlink /usr/local/bin/zuse-grok-auth)" == /usr/local/bin/zuse-cloud-runtime ]]
elif [[ -e /usr/local/bin/zuse-grok-auth ]]; then
  printf '%s\n' unexpected_managed_auth_entry >&2; exit 1
else
  ln -s /usr/local/bin/zuse-cloud-runtime /usr/local/bin/zuse-grok-auth
fi
# A provider may remove its reserved /usr/local/bin/gh on snapshot restore.
# Zuse owns this directory and fixed login-shell PATH entry independently.
python3 - <<'MANAGED_GH'
import os,stat
from pathlib import Path
for p in (Path('/opt'),Path('/opt/zuse'),Path('/opt/zuse/bin'),Path('/etc/profile.d')):
 if not p.exists():p.mkdir(mode=0o755)
 m=p.lstat()
 assert stat.S_ISDIR(m.st_mode) and m.st_uid==0 and not stat.S_IMODE(m.st_mode)&0o022,'managed_gh_ancestor_invalid'
 if p in (Path('/opt/zuse'),Path('/opt/zuse/bin')):
  fd=os.open(p,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
  try:
   current=os.fstat(fd)
   assert (current.st_dev,current.st_ino)==(m.st_dev,m.st_ino),'managed_gh_ancestor_changed'
   os.fchmod(fd,0o755)
  finally:os.close(fd)
 assert p.lstat().st_mode&0o005==0o005,'managed_gh_ancestor_not_traversable'
files={Path('/opt/zuse/bin/gh'):(b'#!/bin/sh\nexec /usr/local/bin/zuse-cloud-runtime github-from-profile "$@"\n',0o755),Path('/etc/profile.d/zz-zuse-managed-bin.sh'):(b'# Zuse managed commands; retain an existing task-specific PATH prefix.\ncase ":${PATH:-}:" in\n  *":/opt/zuse/bin:"*) ;;\n  *) PATH="/opt/zuse/bin:${PATH:-/usr/bin:/bin}" ;;\nesac\nexport PATH\n',0o644)}
for p,(body,mode) in files.items():
 if p.exists():
  m=p.lstat();assert stat.S_ISREG(m.st_mode) and m.st_uid==0 and not stat.S_IMODE(m.st_mode)&0o022,'managed_gh_entry_invalid'
 fd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_TRUNC|os.O_NOFOLLOW,mode)
 try:os.write(fd,body);os.fchmod(fd,mode);os.fchown(fd,0,0);os.fsync(fd)
 finally:os.close(fd)
os.sync()
MANAGED_GH
/opt/zuse/bin/gh --version | head -n 1 | grep -E '^gh version 2\.' >/dev/null
mkdir -p /usr/local/lib/zuse /opt/zuse/harnesses/releases /opt/zuse/harnesses/channels
# These ancestors stay operator-owned but the unprivileged runtime must traverse them.
chmod 0755 /usr/local/lib/zuse /opt/zuse

install -m 755 "$bundle_root/harness-release.py" /usr/local/lib/zuse/harness-release.py
install -m 755 "$bundle_root/launch-runtime.sh" /usr/local/lib/zuse/launch-rust-runtime-core
install -m 755 "$bundle_root/launch-recovery.sh" /usr/local/lib/zuse/launch-rust-recovery
python3 - "$bundle_root" <<'PY'
import hashlib,importlib.util,json,pathlib,shutil,sys
root=pathlib.Path(sys.argv[1]); manifest=json.loads((root/'bundle.json').read_text())
derived=(root/'derivation-validated.json').is_file()
for entry in manifest['files']:
 if derived and entry['path'] in {'claude','codex','grok','codex-assets.tar'}:continue
 p=root/entry['path']
 if hashlib.sha256(p.read_bytes()).hexdigest()!=entry['sha256']: raise SystemExit('bundle_integrity_failed')
harness=manifest['harnessRelease']; dest=pathlib.Path('/opt/zuse/harnesses/releases')/harness
if not derived:
 dest.mkdir()
 for agent in ('claude','codex','grok'):
  shutil.copyfile(root/agent,dest/agent); (dest/agent).chmod(0o555)
 shutil.copyfile(root/'harness-manifest.json',dest/'manifest.json')
spec=importlib.util.spec_from_file_location('harness_release',root/'harness-release.py');manager=importlib.util.module_from_spec(spec);spec.loader.exec_module(manager)
metadata=json.loads((dest/'manifest.json').read_text());assets=metadata['harnesses']['codex'].get('assets')
if assets and not derived:
 manager.install_assets(root/'codex-assets.tar',dest,assets)
 manager.complete_codex(dest,metadata)
elif not assets and (root/'codex-assets.tar').exists():raise SystemExit('unexpected_distribution_archive')
manager.release(pathlib.Path('/opt/zuse/harnesses'),harness,verify=True)
(dest/'manifest.json').chmod(0o444)
pathlib.Path('/opt/zuse/harnesses/channels/staging.json').write_text(json.dumps({'release':harness,'previous':None}))
pathlib.Path('/opt/zuse/harnesses/.release-lock').touch(mode=0o644)
PY
chmod -R a+rX /opt/zuse/harnesses
chmod -R a-w /opt/zuse/harnesses
cat > /usr/local/lib/zuse/launch-rust-runtime <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
export ZUSE_HARNESS_ROOT=/opt/zuse/harnesses ZUSE_HARNESS_CHANNEL=staging
# Agents find what `npm i -g` installed as the task user (see ~/.npmrc).
export PATH="/home/zuse/.local/bin:${PATH:-/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin}"
exec /usr/local/lib/zuse/launch-rust-runtime-core "$@"
EOF
chmod 755 /usr/local/lib/zuse/launch-rust-runtime
printf 'zuse ALL=(ALL) NOPASSWD:ALL\n' > /etc/sudoers.d/zuse
chmod 440 /etc/sudoers.d/zuse
mkdir -p /home/zuse/.zuse-data /home/zuse/workspace /home/zuse/.zuse-image/cache /var/lib/zuse/workspace /var/lib/zuse/recovery /opt/zuse/preparation/secrets /opt/zuse/preparation/status
chmod 0755 /var/lib/zuse
# `npm i -g X` run as zuse (setup scripts, agents) installs under ~/.local:
# no sudo needed, and ~/.local/bin is first on the setup PATH, the runtime PATH
# and login shells. `sudo npm i -g X` keeps root's /usr/local prefix.
mkdir -p /home/zuse/.local/bin
printf 'prefix=/home/zuse/.local\n' > /home/zuse/.npmrc
chmod 644 /home/zuse/.npmrc
printf '%s\n' '# Global npm installs of the task user (prefix in ~/.npmrc).' \
  'case ":${PATH:-}:" in' '  *":$HOME/.local/bin:"*) ;;' '  *) [ -n "${HOME:-}" ] && PATH="$HOME/.local/bin:${PATH:-/usr/bin:/bin}" ;;' 'esac' 'export PATH' \
  > /etc/profile.d/zuse-user-bin.sh
chmod 644 /etc/profile.d/zuse-user-bin.sh

chown -R zuse:zuse /home/zuse /var/lib/zuse/workspace /var/lib/zuse/recovery /opt/zuse/preparation
install -o zuse -g zuse -m 444 "$bundle_root/prepare-plan.json" /opt/zuse/preparation/plan.json
# Publish only after private directory ownership AND the wrapper are ready.
# Otherwise the primary boot process can race root's mkdir/chown and exit.
for helper in runtime-repair.py runtime-recreation.py; do
  install -m 555 "$bundle_root/$helper" "/usr/local/lib/zuse/.$helper.next"
  mv -T "/usr/local/lib/zuse/.$helper.next" "/usr/local/lib/zuse/$helper"
done
install -m 555 "$bundle_root/runtime-supervisor.py" /usr/local/lib/zuse/.runtime-supervisor.next
mv -T /usr/local/lib/zuse/.runtime-supervisor.next /usr/local/lib/zuse/runtime-supervisor.py
# In-place runtime updates (internal-docs/cloud-runtime-updates.md). The root
# helper is pinned as runtimeUpdateHelperSha256; the public key verifies signed
# runtime releases and must equal RELEASE_SIGNING_PUBLIC_KEY in
# crates/protocol/src/cloud_runtime_updates.rs.
if [[ -e "$bundle_root/runtime-update.py" ]]; then
  [[ -f "$bundle_root/runtime-update.py" && ! -L "$bundle_root/runtime-update.py" ]]
  openssl version | grep -E '^OpenSSL [3-9]\.' >/dev/null || { printf '%s\n' runtime_update_openssl_required >&2; exit 1; }
  install -d -o root -g root -m 755 /usr/local/libexec /usr/local/libexec/zuse
  install -m 555 "$bundle_root/runtime-update.py" /usr/local/libexec/zuse/.runtime-update.next
  mv -T /usr/local/libexec/zuse/.runtime-update.next /usr/local/libexec/zuse/runtime-update.py
  install -d -o root -g root -m 755 /usr/local/share/zuse /opt/zuse/runtime /opt/zuse/runtime/releases /var/lib/zuse/runtime-update
  install -d -o root -g root -m 700 /opt/zuse/runtime/.work
  printf '%s\n' 'CTEMp+P0mILHcWFjITtV0yipAeHMNdlho7pxbsntKd8=' > /usr/local/share/zuse/.runtime-release-signing.pub.next
  chmod 644 /usr/local/share/zuse/.runtime-release-signing.pub.next
  mv -T /usr/local/share/zuse/.runtime-release-signing.pub.next /usr/local/share/zuse/runtime-release-signing.pub
fi
# New opt-in image policy is validated without changing ordinary boot behavior.
# Factory preparation still proves an idle resident. The publisher atomically
# stops it and disables ONLY the post-hydration image before snapshot capture.
python3 -I -B -c 'import json,sys;m=json.load(open(sys.argv[1]));assert m.get("managedBaseFreshBootMode") in (None,"post-hydration-first-start-v1") and ("managedBaseFreshBootMode" not in m or m.get("managedBaseFreshBootVersion")==1)' "$bundle_root/bundle.json"
if python3 -I -B -c 'import json,sys;sys.exit(0 if json.load(open(sys.argv[1])).get("managedBaseFreshBootVersion")==1 else 1)' "$bundle_root/bundle.json"; then
  [[ -f "$bundle_root/managed-base-fresh-boot.py" && ! -L "$bundle_root/managed-base-fresh-boot.py" ]]
  install -m 555 "$bundle_root/managed-base-fresh-boot.py" /usr/local/lib/zuse/.managed-base-fresh-boot.next
  mv -T /usr/local/lib/zuse/.managed-base-fresh-boot.next /usr/local/lib/zuse/managed-base-fresh-boot.py
elif [[ -e "$bundle_root/managed-base-fresh-boot.py" ]]; then
  printf '%s\n' fresh_boot_version_required >&2; exit 1
fi
if [[ "$stage" == layout ]]; then
  install -m 555 "$bundle_root/managed-base-install.sh" /usr/local/lib/zuse/managed-base-install.sh
  python3 - <<'LAYOUT_RECEIPT'
import hashlib,json,os,pathlib,stat,tempfile
root=pathlib.Path('/opt/zuse-base-input');out=pathlib.Path('/opt/zuse/preparation/layout-receipt.json')
def digest(path):
 h=hashlib.sha256()
 with path.open('rb') as f:
  while data:=f.read(1024*1024):h.update(data)
 return h.hexdigest()
bundle=json.loads((root/'bundle.json').read_bytes())
value={'version':1,'status':'layout-only','bundleSha256':digest(root/'bundle.json'),'runtimeSha256':digest(pathlib.Path('/usr/local/bin/zuse-cloud-runtime')),'installerSha256':digest(pathlib.Path('/usr/local/lib/zuse/managed-base-install.sh')),'planSha256':digest(pathlib.Path('/opt/zuse/preparation/plan.json')),'harnessRelease':bundle['harnessRelease']}
assert value['runtimeSha256']==bundle['runtimeSha256'],'layout_runtime_mismatch'
fd,name=tempfile.mkstemp(prefix='.layout-',dir=out.parent)
with os.fdopen(fd,'wb') as f:
 os.fchmod(f.fileno(),0o444);f.write(json.dumps(value,sort_keys=True,separators=(',',':')).encode()+b'\n');f.flush();os.fsync(f.fileno())
os.replace(name,out)
fd=os.open(out.parent,os.O_RDONLY|os.O_DIRECTORY)
try:os.fsync(fd)
finally:os.close(fd)
LAYOUT_RECEIPT
fi
fi
if [[ "$stage" == layout ]]; then exit 0; fi
if [[ "$stage" == prepare ]]; then
  python3 - <<'LAYOUT_VERIFY'
import hashlib,json,pathlib,stat
root=pathlib.Path('/opt/zuse-base-input');p=pathlib.Path('/opt/zuse/preparation/layout-receipt.json')
m=p.lstat();assert stat.S_ISREG(m.st_mode) and m.st_uid==0 and not m.st_mode&0o022,'layout_receipt_unsafe'
v=json.loads(p.read_bytes());assert v.get('version')==1 and v.get('status')=='layout-only','layout_receipt_invalid'
def digest(path,owner=0):
 m=path.lstat();assert stat.S_ISREG(m.st_mode) and m.st_uid==owner and not m.st_mode&0o022,'layout_input_unsafe'
 h=hashlib.sha256()
 with path.open('rb') as f:
  while data:=f.read(1024*1024):h.update(data)
 return h.hexdigest()
for key,path in [('bundleSha256',root/'bundle.json'),('runtimeSha256',pathlib.Path('/usr/local/bin/zuse-cloud-runtime')),('installerSha256',pathlib.Path('/usr/local/lib/zuse/managed-base-install.sh')),('planSha256',pathlib.Path('/opt/zuse/preparation/plan.json'))]:
 assert digest(path,10001 if key=='planSha256' else 0)==v[key],'layout_binding_mismatch'
assert json.loads((root/'bundle.json').read_bytes())['harnessRelease']==v['harnessRelease'],'layout_harness_mismatch'
LAYOUT_VERIFY
fi
# Native Boat/Boxd bases use systemd; explicit OCI bases retain their boot command.
if [[ "$stage" == all && -d /run/systemd/system ]]; then
  cat > /etc/systemd/system/zuse-runtime-supervisor.service <<'EOF'
[Unit]
Description=Zuse resident runtime supervisor
After=network.target
[Service]
Type=simple
User=zuse
Group=zuse
UMask=0077
Environment=HOME=/home/zuse
ExecStart=/usr/bin/python3 /usr/local/lib/zuse/runtime-supervisor.py serve
Restart=on-failure
KillMode=mixed
TimeoutStopSec=10
[Install]
WantedBy=multi-user.target
EOF
  chmod 644 /etc/systemd/system/zuse-runtime-supervisor.service
  if [[ -f /usr/local/libexec/zuse/runtime-update.py ]]; then
    # The task user writes request.json (install) or rollback.json (the new
    # runtime failed probation); root swaps the runtime and restarts nothing.
    # The helper trusts nothing it did not copy and verify itself.
    cat > /etc/systemd/system/zuse-runtime-update.path <<'EOF'
[Unit]
Description=Zuse runtime update request
[Path]
PathExists=/home/zuse/.zuse-data/runtime-updates/request.json
PathExists=/home/zuse/.zuse-data/runtime-updates/rollback.json
Unit=zuse-runtime-update.service
[Install]
WantedBy=multi-user.target
EOF
    cat > /etc/systemd/system/zuse-runtime-update.service <<'EOF'
[Unit]
Description=Zuse runtime update (root swap helper)
[Service]
Type=oneshot
User=root
UMask=0022
Environment=PATH=/usr/bin:/bin
ExecStart=/usr/bin/python3 -I -B /usr/local/libexec/zuse/runtime-update.py run
TimeoutStartSec=180
EOF
    cat > /etc/systemd/system/zuse-runtime-update-recover.service <<'EOF'
[Unit]
Description=Zuse runtime update boot recovery
After=local-fs.target
Before=zuse-runtime-supervisor.service zuse-runtime-update.path
[Service]
Type=oneshot
User=root
UMask=0022
Environment=PATH=/usr/bin:/bin
ExecStart=/usr/bin/python3 -I -B /usr/local/libexec/zuse/runtime-update.py recover
[Install]
WantedBy=multi-user.target
EOF
    chmod 644 /etc/systemd/system/zuse-runtime-update.path /etc/systemd/system/zuse-runtime-update.service /etc/systemd/system/zuse-runtime-update-recover.service
  fi
  systemctl daemon-reload
  if [[ -f /usr/local/libexec/zuse/runtime-update.py ]]; then
    systemctl enable zuse-runtime-update-recover.service
    systemctl enable --now zuse-runtime-update.path
  fi
  systemctl enable --now zuse-runtime-supervisor.service
fi
supervisor_ready=false
for _ in {1..100}; do
  if sudo -n -u zuse env HOME=/home/zuse python3 /usr/local/lib/zuse/runtime-supervisor.py ping >/dev/null 2>&1; then
    supervisor_ready=true; break
  fi
  sleep 0.1
done
[[ "$supervisor_ready" == true ]] || { printf '%s\n' base_supervisor_not_ready >&2; exit 1; }
sudo -n -u zuse env HOME=/home/zuse /usr/local/bin/zuse-cloud-runtime prepare-image --plan /opt/zuse/preparation/plan.json
# A copied vendor executable must run with an empty account profile and exact pins.
sudo -n -u zuse env HOME=/home/zuse python3 /usr/local/lib/zuse/harness-release.py --root /opt/zuse/harnesses exec --channel staging --runtime-version "$(/usr/local/bin/zuse-cloud-runtime --version | awk '{print $NF}')" -- /usr/bin/true
# A complete Codex distribution must evaluate local source before capture, without
# any account or model request. Legacy auth-only trees do not pass this base gate.
harness_pin="$(python3 -c 'import json;print(json.load(open("/opt/zuse-base-input/bundle.json"))["harnessRelease"])')"
if [[ -f "$bundle_root/codex-assets.tar" || -f "$bundle_root/derivation-validated.json" ]]; then
  sudo -n -u zuse env HOME=/home/zuse python3 /usr/local/lib/zuse/harness-release.py --root /opt/zuse/harnesses smoke-code-mode --release "$harness_pin"
fi
# Remove transient uploaded files before the provider's snapshot operation.
rm -r -- /opt/zuse-base-input
