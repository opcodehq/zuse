# Native packages and installation

Release downloads include macOS DMGs and archives for Apple silicon and Intel,
a Windows x64 setup executable and portable zip, and Linux `.deb`, `.rpm` and
archives for x64 and ARM64. The release's `SHA256SUMS` covers every asset.

## Install from a Linux terminal

```sh
curl -fsSL https://github.com/opcodehq/zuse/releases/download/v0.26.0-preview.1/install.sh | sh -s -- --version 0.26.0-preview.1
```

This selects the machine's architecture, verifies the archive checksum and
every file in its manifest, then runs the user installer below. It needs Python
3.11+ and keeps profiles and previous package versions. To remove only its
launchers, run the same command with `--uninstall` instead of `--version ...`.
Without `--version`, it selects the latest stable release, falling back to the
latest preview when no stable release exists.

For a system installation, use `sudo apt install ./zuse-VERSION-linux-ARCH.deb`
or `sudo dnf install ./zuse-VERSION-linux-ARCH.rpm`. Replace VERSION and ARCH
with the downloaded filename (ARCH is `x86_64` or `aarch64`). These install under
`/opt/zuse`, register `zuse`, `zuse-server` and `zuse-relay` in `/usr/bin`, and
register the desktop entry. Package managers resolve runtime dependencies.
Remove with `sudo apt remove zuse` or `sudo dnf remove zuse`; profiles remain.
The glibc 2.39/Ubuntu 24.04 build baseline applies to all Linux formats.

## Build local archives

The source checkout is [opcodehq/zuse](https://github.com/opcodehq/zuse).

Build on the target Mac, Linux or Windows machine, with the prerequisites from
[development.md](development.md) and Python 3.11 or newer:

```sh
python3 scripts/package.py
```

On Windows, run `python scripts/package.py` with Python 3.11 or newer and
Git Bash installed.

This builds the release browser and native executables from the checkout, then
creates `dist/build/zuse-VERSION-PLATFORM-ARCH.tar.gz` (a `.zip` on Windows). Keep the terminal open
until the command completes. The output directory must not already contain an
archive with that name. `--profile dev` selects a development native build.
For a preview of the workspace version, use `--version 0.26.0-preview.1`.
The archive and manifest retain that suffix; macOS bundle versions remain numeric.

To assemble executables already built on the target, use:

```sh
python3 scripts/package.py --binaries target/release --assets dist/web \
  --platform linux --arch x86_64 --output dist/build/local
```

The three required executables are `zuse`, `zuse-server`, and `zuse-relay`
(with `.exe` extensions on Windows).
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

For the DMG, open the image and drag **Zuse** to **Applications**. Both the app
and the disk image are signed, notarized and stapled in releases. Choose the
Apple silicon (`aarch64`) or Intel (`x86_64`) download for your Mac.

Extract the archive and copy `Zuse.app` into Applications. The archive also
includes command-line launchers in `bin/`; keep them alongside the app if used.
The bundle includes its icon, URL scheme, microphone description and browser
assets. Local packages are unsigned unless `ZUSE_MACOS_SIGN_IDENTITY` is set;
Apple account credentials additionally enable notarization. Tagged releases
require both signing and notarization on a separate runner.
Gatekeeper acceptance and microphone permission require verification on a Mac.

## Windows

The setup executable installs into `%LOCALAPPDATA%\Programs\Zuse` without
administrator rights, adds a Start menu shortcut, and supports uninstalling
through Windows Settings. User profiles are stored separately and preserved.
The installer is unsigned, like the portable zip below.

Extract the zip anywhere you can write to and run `zuse.exe`. No installer or
administrator rights are needed. `bin\zuse-server.cmd` and `bin\zuse-relay.cmd`
start the helpers in `libexec\` with the packaged browser assets in `web\`;
`--assets` or `ZUSE_WEB_ASSETS` overrides that default. Profiles default to
`%LOCALAPPDATA%\Zuse`.

The release zip is not code-signed, so SmartScreen warns on first launch:
choose **More info**, then **Run anyway**. The embedded browser uses the
[WebView2 Evergreen Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/),
which is usually already installed on Windows 10 and 11. Install it if missing.
The desktop executable also requires the
[Microsoft Visual C++ v14 Redistributable (x64)](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist/);
install it if Windows reports missing `MSVCP140.dll` or `VCRUNTIME140.dll`.
A zip install does not
update itself; the app links to the releases page when a new version is out.

## Provider requirements and profiles

Install Node.js 22 or newer for the SDK model transport. Individual agent CLIs
need their own installation and provider login. Node, agent executables, provider
credentials and optional speech model weights are not bundled. A package does
not establish live-provider or multi-device acceptance.

Desktop profiles default to `~/.zuse-native` on macOS/Linux and
`%LOCALAPPDATA%\Zuse` on Windows, and can be selected with `ZUSE_DATA_DIR`.
The packager copies only explicit executables, web assets and legal files; it
does not copy development profiles, environment files or credentials.

## Release formats

The release workflow calls `distributions.yml` after publishing the original
archives. Existing archives are verified and reused; missing Intel macOS or
Linux ARM64 archives build from the immutable release tag with the matching
published browser assets. Signing runs on separate fresh macOS runners.
`scripts/distribution.py` converts verified archives to native installers.

To add formats to an existing release without rebuilding its existing binaries:

```sh
gh workflow run release.yml -f tag=v0.26.0-preview.1 -f formats_only=true
```

This adds the downloads and updates checksums and installation notes on the
same release. The version tag stays unchanged.
