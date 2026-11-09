# Control icons

Native and browser clients share the embedded assets in `zuse-icons`. Ordinary builds use the bundled Hugeicons free rounded set, pinned to `@hugeicons/core-free-icons` 4.2.1. Provider logos and file-identity artwork keep their existing assets.

When `HUGEICONS_LICENSE_KEY` is present in the build environment, Cargo prepares Hugeicons Pro solid rounded glyphs automatically using Node.js 22. The key is used only for the private registry request. It is not written to npm configuration, SVGs, provenance, or app code. A configured but invalid key fails the build instead of silently changing the selected style. When no key is configured, the free build needs no registry access or Node icon-generation step.

Set `HUGEICONS_SOLID_VERSION` to pin a licensed package version; otherwise the licensed registry's current version is used. Generated provenance records its resolved version and SHA-512 integrity. Licensed source vectors remain in Cargo's output directory. CI reads the optional `HUGEICONS_LICENSE_KEY` Actions secret and `HUGEICONS_SOLID_VERSION` repository variable for native and browser builds.

For local builds, use `python3 scripts/cargo_dev.py` in place of `cargo`, or the
native/browser dev launchers. They compute `HUGEICONS_BUILD_KEY`, a non-secret
digest that lets Cargo detect key changes without rebuilding unchanged icons
and their UI dependents on every command. CI computes the same digest before
building. Bare Cargo still detects the key automatically, but reruns the icon
selector on each invocation. Do not set `HUGEICONS_BUILD_KEY` manually: the
launcher must recompute it when the license changes.

To regenerate the committed free assets:

```sh
node scripts/prepare-icons.mjs --output crates/icons/assets/free
```

Run this without a license key in the environment. The generator verifies the package's SHA-512 integrity, imports only mapped icon data, and writes the compact SVG subset. It does not install a JavaScript renderer or run npm lifecycle scripts.

License activation and package details: https://hugeicons.com/docs/license/activate-license.
