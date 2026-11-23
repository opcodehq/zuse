import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createLogoResolver } from "./logos";

const table = readFileSync(new URL("../../../../../../crates/icons/src/plugins.rs", import.meta.url), "utf8");
const logoUrl = createLogoResolver(table, {
  "assets/linear.png": "/linear.png",
  "assets/better-stack.jpg": "/better-stack.jpg",
});

test("resolves bundled logos including block-form Rust match arms", () => {
  assert.equal(logoUrl("linear.app"), "/linear.png");
  assert.equal(logoUrl("betterstack.com"), "/better-stack.jpg");
  assert.equal(logoUrl(" LINEAR.APP "), "/linear.png");
});

test("resolves missing catalog images on demand", () => {
  assert.equal(logoUrl("ramp.com"), "https://www.google.com/s2/favicons?domain=ramp.com&sz=128");
  assert.ok(logoUrl("example.com")?.includes("domain=example.com"));
});

test("does not forward custom server URLs or credentials", () => {
  for (const domain of ["", "localhost", "https://example.com/private", "user:secret@example.com", "example.com?token=secret"]) {
    assert.equal(logoUrl(domain), null);
  }
});
