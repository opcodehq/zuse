# Control icons

Native and browser clients share the embedded assets in `zuse-icons`. Ordinary builds use the bundled Hugeicons free rounded set, pinned to `@hugeicons/core-free-icons` 4.2.1. Provider logos and file-identity artwork keep their existing assets.

When `HUGEICONS_LICENSE_KEY` is present in the build environment, Cargo prepares Hugeicons Pro solid rounded glyphs automatically using Node.js 22. The key is used only for the private registry request. It is not written to npm configuration, SVGs, provenance, or app code. A configured but invalid key fails the build instead of silently changing the selected style. When no key is configured, the free build needs no registry access or Node icon-generation step.

Set `HUGEICONS_SOLID_VERSION` to pin a licensed package version; otherwise the licensed registry's current version is used. Generated provenance records its resolved version and SHA-512 integrity. Licensed source vectors remain in Cargo's output directory. CI reads the optional `HUGEICONS_LICENSE_KEY` Actions secret and `HUGEICONS_SOLID_VERSION` repository variable for native and browser builds.

To regenerate the committed free assets:

```sh
node scripts/prepare-icons.mjs --output crates/icons/assets/free
```

Run this without a license key in the environment. The generator verifies the package's SHA-512 integrity, imports only mapped icon data, and writes the compact SVG subset. It does not install a JavaScript renderer or run npm lifecycle scripts.

License activation and package details: https://hugeicons.com/docs/license/activate-license.
