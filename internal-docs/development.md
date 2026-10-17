# Development environment

Use stable Rust, rustfmt and Clippy. Keep versions in the workspace and lockfile. Browser builds additionally require the `wasm32-unknown-unknown` target and Trunk.

## Linux native dependencies

The native platform needs a C/C++ compiler, pkg-config, CMake, OpenSSL, Fontconfig, FreeType, XCB, XKBCommon/X11, Wayland, Vulkan loader, ALSA and Clang development files. Embedded browser previews additionally require WebKitGTK 4.1 and JSON-GLib development files. Package names vary by distribution.

The cloud development VM runs Amazon Linux 2023. WebKitGTK availability must be resolved for its native preview build; lack of a package must not silently produce a nonfunctional preview feature.

## Validation policy

Use `cargo check` for integration feedback. Keep tests centered on observable behavior and run relevant suites at milestones. Run native and browser acceptance separately: a web screenshot does not verify platform-specific desktop integration.
