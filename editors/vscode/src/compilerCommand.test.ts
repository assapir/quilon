// Unit tests for compiler resolution. The probe is injected, so these run under
// plain Node against a make-believe machine: `pnpm test` compiles to out/ then
// `node --test`.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  type CompilerProbe,
  missingCompilerMessage,
  resolveCompilerCommand,
  shellCommand,
} from "./compilerCommand";

/** A probe over a fixed set of executable files and readable texts. */
function probe(
  options: Partial<CompilerProbe> & { files?: string[]; texts?: Record<string, string> } = {},
): CompilerProbe {
  const files = new Set(options.files ?? []);
  const texts = options.texts ?? {};
  return {
    configured: options.configured,
    path: options.path,
    pathExt: options.pathExt,
    home: options.home,
    workspaceFolders: options.workspaceFolders,
    platform: options.platform ?? "linux",
    isExecutable: (file) => files.has(file),
    readText: (file) => texts[file],
  };
}

const QUILON_CRATE = '[package]\nname = "quilon"\nversion = "0.9.2"\n';

test("an explicitly configured command wins, split into exe and base args", () => {
  const resolved = resolveCompilerCommand(
    probe({ configured: "  cargo run -- ", files: ["/usr/bin/quilon"], path: "/usr/bin" }),
  );
  assert.deepEqual(resolved, { exe: "cargo", baseArgs: ["run", "--"], origin: "configured" });
});

test("a configured bare `quilon` still gets the search (it says nothing the default doesn't)", () => {
  const resolved = resolveCompilerCommand(
    probe({
      configured: "quilon",
      path: "/usr/bin",
      home: "/home/u",
      files: ["/home/u/.cargo/bin/quilon"],
    }),
  );
  assert.deepEqual(resolved, {
    exe: "/home/u/.cargo/bin/quilon",
    baseArgs: [],
    origin: "install-dir",
  });
});

test("a configured path to a compiler is used verbatim, search or no search", () => {
  const resolved = resolveCompilerCommand(
    probe({ configured: "/opt/quilon/bin/quilon", path: "/usr/bin", files: ["/usr/bin/quilon"] }),
  );
  assert.deepEqual(resolved, {
    exe: "/opt/quilon/bin/quilon",
    baseArgs: [],
    origin: "configured",
  });
});

test("a compiler on PATH is used as a full path", () => {
  const resolved = resolveCompilerCommand(
    probe({ path: "/opt/nope:/usr/local/bin", files: ["/usr/local/bin/quilon"] }),
  );
  assert.deepEqual(resolved, { exe: "/usr/local/bin/quilon", baseArgs: [], origin: "path" });
});

test("a cargo-installed compiler is found even when PATH misses it (the GUI-launch case)", () => {
  const resolved = resolveCompilerCommand(
    probe({ path: "/usr/bin", home: "/home/u", files: ["/home/u/.cargo/bin/quilon"] }),
  );
  assert.deepEqual(resolved, {
    exe: "/home/u/.cargo/bin/quilon",
    baseArgs: [],
    origin: "install-dir",
  });
});

test("PATH takes precedence over the install directories", () => {
  const resolved = resolveCompilerCommand(
    probe({
      path: "/usr/bin",
      home: "/home/u",
      files: ["/usr/bin/quilon", "/home/u/.cargo/bin/quilon"],
    }),
  );
  assert.equal(resolved.exe, "/usr/bin/quilon");
});

test("a checkout's release build is used before its debug build", () => {
  const resolved = resolveCompilerCommand(
    probe({
      workspaceFolders: ["/w/quilon"],
      files: ["/w/quilon/target/release/quilon", "/w/quilon/target/debug/quilon"],
    }),
  );
  assert.deepEqual(resolved, {
    exe: "/w/quilon/target/release/quilon",
    baseArgs: [],
    origin: "checkout-binary",
  });
});

test("an unbuilt compiler checkout falls back to `cargo run --quiet --`", () => {
  const resolved = resolveCompilerCommand(
    probe({
      path: "/home/u/.cargo/bin",
      home: "/home/u",
      workspaceFolders: ["/w/quilon"],
      files: ["/home/u/.cargo/bin/cargo"],
      texts: { "/w/quilon/Cargo.toml": QUILON_CRATE },
    }),
  );
  assert.deepEqual(resolved, {
    exe: "/home/u/.cargo/bin/cargo",
    baseArgs: ["run", "--quiet", "--"],
    origin: "cargo",
  });
});

test("cargo is not offered for a workspace that is some other Rust crate", () => {
  const resolved = resolveCompilerCommand(
    probe({
      home: "/home/u",
      workspaceFolders: ["/w/other"],
      files: ["/home/u/.cargo/bin/cargo"],
      texts: { "/w/other/Cargo.toml": '[package]\nname = "other"\n' },
    }),
  );
  assert.equal(resolved.origin, "fallback");
});

test("nothing found falls back to a bare `quilon`", () => {
  assert.deepEqual(resolveCompilerCommand(probe({ path: "/usr/bin" })), {
    exe: "quilon",
    baseArgs: [],
    origin: "fallback",
  });
});

test("Windows: PATHEXT extensions and `;` separators are honored", () => {
  const resolved = resolveCompilerCommand(
    probe({
      platform: "win32",
      path: "C:\\nope;C:\\tools",
      pathExt: ".COM;.EXE",
      files: ["C:\\tools\\quilon.EXE"],
    }),
  );
  assert.deepEqual(resolved, { exe: "C:\\tools\\quilon.EXE", baseArgs: [], origin: "path" });
});

test("missingCompilerMessage: a configured command is named as the setting's", () => {
  const message = missingCompilerMessage({ exe: "quilonc", baseArgs: [], origin: "configured" });
  assert.match(message, /"quilonc"/);
  assert.match(message, /quilon\.command/);
});

test("missingCompilerMessage: an unresolved compiler says where we looked", () => {
  const message = missingCompilerMessage({ exe: "quilon", baseArgs: [], origin: "fallback" });
  assert.match(message, /no Quilon compiler found/);
  assert.match(message, /cargo install --path \./);
});

test("shellCommand: joins the invocation and quotes only what has whitespace", () => {
  assert.equal(
    shellCommand({ exe: "/home/a b/quilon", baseArgs: [], origin: "install-dir" }),
    '"/home/a b/quilon"',
  );
  assert.equal(
    shellCommand({ exe: "cargo", baseArgs: ["run", "--quiet", "--"], origin: "cargo" }),
    "cargo run --quiet --",
  );
});
