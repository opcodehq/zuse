#!/usr/bin/env bash
# Lays out Zuse's managed base inside a Prized sandbox, as the user `prized`
# (no root, no sudo). Run by build.py with the uploaded input directory, whose
# files build.py has already checked against inputs.sha256. Idempotent.
#
# Layout (the worker depends on these paths; see crates/cloud/src/runtime/machine_layout.rs):
#   R=/home/prized/.zuse   bin/ lib/ harnesses/ status/ recovery/ boot/ state/ preparation/
#   /home/prized/.zuse-data (runtime data), /home/prized/workspace (empty in the base)
set -euo pipefail
umask 077
input="${1:?input directory required}"
H=/home/prized
R="$H/.zuse"
[[ "$(id -un)" == prized && "$HOME" == "$H" && -f "$input/build.json" ]]
harness="$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["harnessRelease"])' "$input/build.json")"
[[ "$harness" =~ ^[0-9a-f]{64}$ ]]
(cd "$input" && sha256sum -c --quiet inputs.sha256)

mkdir -p "$R"/{bin,lib,libexec,status,recovery,boot,state,preparation/status,preparation/secrets,harnesses/releases,harnesses/channels} \
  "$H/.zuse-data" "$H/workspace" "$H/.zuse-image/cache"
chmod 700 "$R"/{status,recovery,boot,state,preparation/secrets} "$H/.zuse-data"
# Traversable like the root base's /usr/local/lib/zuse and /opt/zuse.
chmod 755 "$R" "$R"/{bin,lib,libexec,harnesses,harnesses/releases,harnesses/channels}

# Runtime binary (+ the managed auth entry the root base also provides).
install -m 755 "$input/runtime" "$R/bin/zuse-cloud-runtime"
ln -sfn zuse-cloud-runtime "$R/bin/zuse-grok-auth"

# Node.js LTS: the exact release and SHA-256 the root base pins in
# managed-base-install.sh (the provider image's /usr/bin/node is an older line).
# Unpacked under R/libexec and linked into ~/.local/bin, the first PATH entry
# for agents, repository and machine setup scripts, and login shells. npm's
# global prefix stays ~/.local, so `npm i -g` tools land next to it.
node_version="$(sed -n 's/^node_version=\([0-9][0-9.]*\)$/\1/p' "$input/repo/managed-base-install.sh")"
node_sha256="$(sed -n 's/^node_sha256=\([0-9a-f]\{64\}\)$/\1/p' "$input/repo/managed-base-install.sh")"
[[ "$node_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && "$node_sha256" =~ ^[0-9a-f]{64}$ ]] \
  || { printf '%s\n' node_pin_missing >&2; exit 1; }
node_dir="$R/libexec/node-v$node_version"
if [[ ! -x "$node_dir/bin/node" ]]; then
  node_work="$(mktemp -d "$H/.zuse-node.XXXXXX")"
  python3 -I -B - "$node_version" "$node_sha256" "$node_work/node.tar.xz" <<'PY'
import hashlib,sys,urllib.request
version,expected,target=sys.argv[1:4]
url=f'https://nodejs.org/dist/v{version}/node-v{version}-linux-x64.tar.xz'
with urllib.request.urlopen(urllib.request.Request(url,headers={'User-Agent':'zuse-base-builder'}),timeout=300) as r:
    data=r.read((200<<20)+1)
assert len(data)<=200<<20,'node_download_too_large'
assert hashlib.sha256(data).hexdigest()==expected,'node_checksum_mismatch'
with open(target,'wb') as out:out.write(data)
PY
  tar -xJf "$node_work/node.tar.xz" -C "$node_work" --no-same-owner
  chmod -R u+rwX,go+rX,go-w "$node_work/node-v$node_version-linux-x64"
  mv -T "$node_work/node-v$node_version-linux-x64" "$node_dir"
  rm -rf -- "$node_work"
fi
mkdir -p "$H/.local/bin"
chmod 755 "$H/.local" "$H/.local/bin"
for tool in node npm npx corepack; do
  ln -sfn "$node_dir/bin/$tool" "$H/.local/bin/$tool"
done
node_check="$(env -i HOME="$H" PATH="$H/.local/bin:/usr/local/bin:/usr/bin:/bin" \
  /bin/bash --noprofile --norc -ec 'node --version; npm --version >/dev/null; npx --version >/dev/null; corepack --version >/dev/null; npm prefix -g')" \
  || { printf '%s\n' node_task_user_unavailable >&2; exit 1; }
[[ "$node_check" == "v$node_version"$'\n'"$H/.local" ]] || { printf '%s\n' node_task_user_unexpected >&2; exit 1; }
[[ "$(bash -lc 'node --version')" == "v$node_version" ]] || { printf '%s\n' node_login_path_invalid >&2; exit 1; }
rm -rf -- "$H/.npm"

# Supervisor and its sibling modules must share one directory.
for helper in runtime-supervisor.py runtime-repair.py runtime-recreation.py; do
  install -m 555 "$input/repo/$helper" "$R/lib/.$helper.next"
  mv -T "$R/lib/.$helper.next" "$R/lib/$helper"
done
install -m 755 "$input/repo/harness-release.py" "$R/lib/harness-release.py"
install -m 755 "$input/repo/launch-runtime.sh" "$R/lib/launch-rust-runtime"
# The repository launcher defaults its supervisor to /usr/local/lib/zuse; the
# worker runs recovery directly, so a thin wrapper supplies the relocated paths.
mkdir -p -m 755 "$R/lib/recovery-core"
install -m 755 "$input/repo/launch-recovery.sh" "$R/lib/recovery-core/launch-rust-recovery"
install -m 755 "$input/local/launch-rust-recovery" "$R/lib/launch-rust-recovery"
install -m 755 "$input/local/zuse-supervise" "$R/lib/zuse-supervise"
install -m 755 "$input/local/zuse-launch" "$R/lib/zuse-launch"

# Harness release, channel "staging" (as managed-base-install.sh, minus root/chown).
[[ ! -d "$R/harnesses" ]] || chmod -R u+w "$R/harnesses"
python3 -I -B - "$input" "$R/harnesses" "$harness" <<'PY'
import hashlib,importlib.util,json,pathlib,shutil,sys,tempfile
source,root,harness=pathlib.Path(sys.argv[1]),pathlib.Path(sys.argv[2]),sys.argv[3]
spec=importlib.util.spec_from_file_location('harness_release',source/'repo/harness-release.py')
manager=importlib.util.module_from_spec(spec);spec.loader.exec_module(manager)
manifest_bytes=(source/'harness-manifest.json').read_bytes()
if hashlib.sha256(manager.canonical(json.loads(manifest_bytes))).hexdigest()!=harness:raise SystemExit('harness_manifest_mismatch')
dest=root/'releases'/harness
if not dest.exists():
    staged=pathlib.Path(tempfile.mkdtemp(prefix='.candidate-',dir=root/'releases'))
    for agent in ('claude','codex','grok'):
        shutil.copyfile(source/agent,staged/agent);(staged/agent).chmod(0o555)
    shutil.copyfile(source/'harness-manifest.json',staged/'manifest.json')
    metadata=json.loads((staged/'manifest.json').read_text());assets=metadata['harnesses']['codex'].get('assets')
    if assets:
        manager.install_assets(source/'codex-assets.tar',staged,assets)
        manager.complete_codex(staged,metadata)
    staged.chmod(0o755)
    staged.rename(dest)
manager.release(root,harness,verify=True)
(dest/'manifest.json').chmod(0o444)
manager.atomic_json(root/'channels'/'staging.json',{'release':harness,'previous':None})
(root/'.release-lock').touch(mode=0o644)
print('harness release',harness)
PY
chmod -R a+rX "$R/harnesses"
chmod -R a-w "$R/harnesses"

# GitHub CLI: Prized's image has none. Install a checksum-verified gh 2.x
# release, reached through a managed R/bin/gh wrapper on the login-shell PATH.
if [[ ! -x "$R/libexec/gh" ]]; then
  if command -v gh >/dev/null 2>&1 && [[ "$(command -v gh)" != "$R/bin/gh" ]]; then
    install -m 755 "$(command -v gh)" "$R/libexec/gh"
  else
    python3 -I -B - "$R/libexec" "${ZUSE_GH_VERSION:-}" <<'PY'
import hashlib,io,json,os,re,sys,tarfile,urllib.request
target,pinned=sys.argv[1],sys.argv[2]
def fetch(url,limit):
    with urllib.request.urlopen(urllib.request.Request(url,headers={'User-Agent':'zuse-base-builder'}),timeout=120) as r:
        data=r.read(limit+1)
    assert len(data)<=limit,'gh_download_too_large';return data
if pinned:version=pinned
else:version=json.loads(fetch('https://api.github.com/repos/cli/cli/releases/latest',1<<20))['tag_name'].lstrip('v')
assert re.fullmatch(r'2\.[0-9]+\.[0-9]+',version),'gh_version_not_2x'
base=f'https://github.com/cli/cli/releases/download/v{version}/'
name=f'gh_{version}_linux_amd64.tar.gz'
sums=fetch(base+f'gh_{version}_checksums.txt',1<<20).decode()
expected=[l.split()[0] for l in sums.splitlines() if l.split()[1:]==[name]]
assert len(expected)==1 and re.fullmatch(r'[0-9a-f]{64}',expected[0]),'gh_checksum_missing'
archive=fetch(base+name,200<<20)
assert hashlib.sha256(archive).hexdigest()==expected[0],'gh_checksum_mismatch'
with tarfile.open(fileobj=io.BytesIO(archive),mode='r:gz') as tar:
    member=tar.getmember(f'gh_{version}_linux_amd64/bin/gh')
    assert member.isfile(),'gh_member_invalid'
    data=tar.extractfile(member).read()
temporary=os.path.join(target,'.gh.next')
with open(temporary,'wb') as out:out.write(data)
os.chmod(temporary,0o755);os.replace(temporary,os.path.join(target,'gh'))
print('gh',version,expected[0])
PY
  fi
fi
# Managed gh, exactly as the runtime's home layout expects (crates/cloud-runtime
# src/github/cli.rs HOME_LAYOUT): the real CLI in libexec, a wrapper that routes
# through the runtime's brokered sign-in, and a login-shell profile.
[[ -f "$R/libexec/gh" && ! -L "$R/libexec/gh" ]] || { printf '%s\n' managed_gh_not_regular >&2; exit 1; }
chmod 755 "$R/libexec/gh"
printf '#!/bin/sh\nexec /home/prized/.zuse/bin/zuse-cloud-runtime github-from-profile "$@"\n' > "$R/bin/.gh.next"
chmod 755 "$R/bin/.gh.next"
mv -T "$R/bin/.gh.next" "$R/bin/gh"
mkdir -p "$R/profile.d"
chmod 755 "$R" "$R/bin" "$R/profile.d"
cat > "$R/profile.d/.managed.next" <<'PROFILE'
# Zuse managed commands; retain an existing task-specific PATH prefix.
case ":${PATH:-}:" in
  *":/home/prized/.zuse/bin:"*) ;;
  *) PATH="/home/prized/.zuse/bin:${PATH:-/usr/bin:/bin}" ;;
esac
export PATH
PROFILE
chmod 644 "$R/profile.d/.managed.next"
mv -T "$R/profile.d/.managed.next" "$R/profile.d/zz-zuse-managed-bin.sh"
"$R/bin/gh" --version | head -n 1 | grep -E '^gh version 2\.' >/dev/null
python3 -I -B - "$R/profile.d/zz-zuse-managed-bin.sh" "$H/.profile" "$H/.bashrc" <<'PY'
import os,sys
profile=sys.argv[1]
begin='# >>> zuse managed commands >>>';end='# <<< zuse managed commands <<<'
block=f'{begin}\n[ -r {profile} ] && . {profile}\n{end}\n'
for path in sys.argv[2:]:
    text=open(path).read() if os.path.exists(path) else ''
    if begin in text:
        head,rest=text.split(begin,1);text=head+rest.split(end+'\n',1)[1]
    # First in .bashrc, before its early return for non-interactive shells.
    text=block+text if path.endswith('.bashrc') else text.rstrip('\n')+'\n\n'+block
    temporary=path+'.zuse-next'
    with open(temporary,'w') as out:out.write(text)
    os.chmod(temporary,0o644);os.replace(temporary,path)
PY
bash -lc 'command -v gh' | head -n 1 | grep -qx "$R/bin/gh" || { printf '%s\n' managed_gh_login_path_invalid >&2; exit 1; }

# Preparation plan and the idle resident supervisor prepare-image requires.
install -m 444 "$input/plan.json" "$R/preparation/plan.json"
[[ -z "$(ls -A "$H/workspace")" ]] || { printf '%s\n' workspace_not_empty >&2; exit 1; }
"$R/lib/zuse-supervise"
manifest="$("$R/bin/zuse-cloud-runtime" prepare-image --plan "$R/preparation/plan.json")"
# A copied vendor executable must run with an empty account profile and exact pins.
runtime_version="$("$R/bin/zuse-cloud-runtime" --version | awk '{print $NF}')"
python3 "$R/lib/harness-release.py" --root "$R/harnesses" exec --channel staging \
  --runtime-version "$runtime_version" -- /usr/bin/true
python3 "$R/lib/harness-release.py" --root "$R/harnesses" smoke-code-mode --release "$harness"
printf 'ZUSE_IMAGE_MANIFEST %s\n' "$manifest"
