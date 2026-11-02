# Local native packages

The source checkout is [opcodehq/zuse](https://github.com/opcodehq/zuse).

Build on the target Mac or Linux machine, with the prerequisites from
[development.md](development.md) and Python 3.11 or newer:

```sh
python3 scripts/package.py
```

This builds the release browser and native executables from the checkout, then
creates `dist/build/zuse-VERSION-PLATFORM-ARCH.tar.gz`. Keep the terminal open
until the command completes. The output directory must not already contain an
archive with that name. `--profile dev` selects a development native build.

To assemble executables already built on the target, use:

```sh
python3 scripts/package.py --binaries target/release --assets dist/web \
  --platform linux --arch x86_64 --output dist/build/local
```

The three required executables are `zuse`, `zuse-server`, and `zuse-relay`.
The packager checks their binary platform and architecture. Prebuilt inputs
are labelled as such; their source revision and optimization level are not
inferred. Browser assets must come from the matching source/protocol build.
Assembly reads toolkit legal notices from the pinned Cargo checkout or a matching
`vendor/zkit` checkout; run `cargo fetch --locked` if neither is available.
Packages include a SHA-256 file inventory, the root license, dependency lockfile
and retained component/font/icon/SDK notices. The inventory detects accidental
changes; it is not a signature or a completed dependency-license audit.

## Linux

Extract the archive, enter its directory, then run:

```sh
python3 install.py
```

The installer requires Python 3.11 or newer and a matching CPU architecture.

The installer copies the package into `~/.local/share/zuse/packages`, adds
launchers to `~/.local/bin`, and registers a desktop entry and `zuse://` URL
handler for the current user. Add `~/.local/bin` to PATH if needed. Desktop
environments may require a session refresh to discover the entry. No sudo or
system service is needed. Set `XDG_DATA_HOME` or `ZUSE_BIN_HOME` before installing
to choose other locations. Existing unrelated launchers are never replaced.
Upgrades activate the new package and retain the previous package directory.

Run `python3 install.py --uninstall` with the same environment to remove the
Zuse-owned launch/desktop links. Package directories and profiles are preserved.
Stop running instances and any daemon you enabled before removing old package
directories manually. No daemon is enabled automatically.

You can also run `./bin/zuse` directly from any extracted location. `bin/zuse-server`
and `bin/zuse-relay` locate their packaged browser assets regardless of the current
directory; `--assets` overrides that default. Native Browser Access settings also
discover the installed assets. `ZUSE_WEB_ASSETS` can override asset discovery.

Linux requires compatible system libraries, including WebKitGTK 4.1, JSON-GLib,
OpenSSL, ALSA, X11/Wayland and a working Vulkan GPU driver. Build on the oldest
distribution you intend to support; an archive is not a portable static binary.
Xvfb/software rendering checks do not establish physical GPU or Wayland support.

## macOS

Extract the archive and copy `Zuse.app` into Applications. The archive also
includes command-line launchers in `bin/`; keep them alongside the app if used.
The bundle includes its icon, URL scheme, microphone description and browser
assets. A local package is unsigned: signing, hardened-runtime entitlements and
notarization are separate release steps and are not performed by this command.
Gatekeeper acceptance and microphone permission require verification on a Mac.

## Provider requirements and profiles

Install Node.js 22 or newer for the SDK model transport. Individual agent CLIs
need their own installation and provider login. Node, agent executables, provider
credentials and optional speech model weights are not bundled. A package does
not establish live-provider or multi-device acceptance.

Profiles default to `~/.zuse-native` and can be selected with `ZUSE_DATA_DIR`.
The packager copies only explicit executables, web assets and legal files; it
does not copy development profiles, environment files or credentials.
