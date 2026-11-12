// Builds crates/web-link to WebAssembly for the paired transport.
// Output goes to src/features/connection/link/pkg (ignored by git), which Vite bundles when present.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, renameSync, statSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "../../..");
const out = join(here, "../src/features/connection/link/pkg");
const targetDir = join(repo, "target/web-link");
const wasm = join(targetDir, "wasm32-unknown-unknown/release/zuse_web_link.wasm");

function bindgenVersion() {
  const lock = readFileSync(join(repo, "Cargo.lock"), "utf8");
  return lock.match(/name = "wasm-bindgen"\nversion = "([^"]+)"/)?.[1];
}

function find(name, candidates) {
  for (const candidate of candidates) if (candidate && existsSync(candidate)) return candidate;
  try {
    return execFileSync("which", [name], { encoding: "utf8" }).trim() || null;
  } catch {
    return null;
  }
}

function fail(message) {
  // An earlier build keeps working; without one the app reports that paired connections are unavailable.
  if (existsSync(join(out, "zuse_web_link.js"))) {
    console.warn(`web-link: ${message}; keeping the previous build`);
    process.exit(0);
  }
  console.warn(`web-link: ${message}; paired connections will be unavailable in this build`);
  process.exit(0);
}

const version = bindgenVersion();
const bindgen = find("wasm-bindgen", [process.env.ZUSE_WASM_BINDGEN, version && join(homedir(), `.cache/trunk/wasm-bindgen-${version}/wasm-bindgen`)]);
if (!bindgen) fail(`wasm-bindgen ${version ?? ""} was not found (set ZUSE_WASM_BINDGEN)`);

try {
  execFileSync("cargo", ["build", "-p", "zuse-web-link", "--target", "wasm32-unknown-unknown", "--release", "--target-dir", targetDir], {
    cwd: repo,
    stdio: ["ignore", "ignore", "inherit"],
    env: {
      ...process.env,
      CARGO_PROFILE_RELEASE_OPT_LEVEL: "z",
      CARGO_PROFILE_RELEASE_LTO: "fat",
      CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "1",
      CARGO_PROFILE_RELEASE_PANIC: "abort",
    },
  });
} catch {
  fail("cargo could not build crates/web-link for wasm32-unknown-unknown");
}

const built = join(out, "zuse_web_link_bg.wasm");
if (existsSync(built) && statSync(built).mtimeMs >= statSync(wasm).mtimeMs) process.exit(0);

const staging = `${out}.tmp-${process.pid}`;
rmSync(staging, { recursive: true, force: true });
mkdirSync(staging, { recursive: true });
execFileSync(bindgen, ["--target", "web", "--no-typescript", "--out-dir", staging, wasm], { stdio: "inherit" });
const opt = find("wasm-opt", [process.env.ZUSE_WASM_OPT, join(homedir(), ".cache/trunk/wasm-opt-version_123/bin/wasm-opt")]);
if (opt) {
  const file = join(staging, "zuse_web_link_bg.wasm");
  execFileSync(opt, ["-Oz", "--all-features", "-o", `${file}.opt`, file], { stdio: "inherit" });
  renameSync(`${file}.opt`, file);
}
rmSync(out, { recursive: true, force: true });
renameSync(staging, out);
console.log("web-link: built", out);
