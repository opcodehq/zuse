#!/usr/bin/env python3
"""Add installer download and terminal installation instructions to release notes."""
from pathlib import Path
import sys
from distribution import release_version

version = release_version(sys.argv[1])
# Releases before the standalone CLI archive regenerate notes without it.
cli_archives = '--cli-archives' in sys.argv[3:]
tag = 'v' + version
old = Path(sys.argv[2]).read_text()
notice = 'This is a preview release for testing.' if '-' in version else ''
start, end = '<!-- zuse-installers:start -->', '<!-- zuse-installers:end -->'
if start in old and end in old:
    old = old[:old.index(start)] + old[old.index(end) + len(end):]
# Replace the original three-platform download preface, retaining the changelog.
if old.startswith('Download the package for your computer below.') and '### Added' in old:
    old = old[old.index('### Added'):]
cli_section = f'''### Command line only

For servers and other machines without a display, `zuse-cli-{version}-<platform>-<arch>.tar.gz` (`.zip` on Windows) holds just the `zuse` command and the browser app it serves, with no desktop libraries required. Extract it and run `bin/zuse serve --lan` or `bin/zuse serve --account`; `bin/zuse-server` keeps the previous command name working.

''' if cli_archives else ''
print(f'''{start}
## Downloads and installation

| Platform | Downloads |
| --- | --- |
| macOS Apple silicon | [DMG]({f'https://github.com/opcodehq/zuse/releases/download/{tag}/zuse-{version}-macos-aarch64.dmg'}) · `.tar.gz` |
| macOS Intel | [DMG]({f'https://github.com/opcodehq/zuse/releases/download/{tag}/zuse-{version}-macos-x86_64.dmg'}) · `.tar.gz` |
| Windows x64 | [Setup EXE]({f'https://github.com/opcodehq/zuse/releases/download/{tag}/zuse-{version}-windows-x86_64-setup.exe'}) · portable `.zip` |
| Linux x64 | `.deb` · `.rpm` · `.tar.gz` |
| Linux ARM64 | `.deb` · `.rpm` · `.tar.gz` |

macOS: open the signed, notarized DMG and drag **Zuse** to **Applications**.
Windows: run Setup for a per-user installation with Start menu shortcuts and uninstall support. Windows downloads remain unsigned. Install the [Visual C++ v14 Redistributable (x64)](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist/) and [WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) if missing.

### Install from a Linux terminal

```sh
(
  set -e
  installer=$(mktemp)
  trap 'rm -f "$installer"' EXIT
  curl -fsSL https://github.com/opcodehq/zuse/releases/download/{tag}/install.sh -o "$installer"
  sh "$installer" --version {version}
)
```

The installer requires Python 3.11+, selects x64 or ARM64, verifies SHA-256 checksums, and installs into `~/.local/share/zuse` with launchers in `~/.local/bin`. It preserves profiles and existing package versions. No sudo is required for the archive installation. Linux needs glibc 2.39+, WebKitGTK 4.1 and a Vulkan driver; Ubuntu 24.04+ is the build baseline. System packages let apt/dnf resolve dependencies:

```sh
sudo apt install ./zuse-{version}-linux-x86_64.deb
sudo dnf install ./zuse-{version}-linux-x86_64.rpm
```

For ARM64, use the `linux-aarch64` package. To remove only the terminal install's launchers, rerun the shell installer with `--uninstall`; profiles are retained. System packages are removed with apt/dnf. Agent CLIs and provider logins are separate.

{cli_section}`SHA256SUMS` covers every download, including `install.sh`. {notice}
{end}

{old.strip()}
''')
