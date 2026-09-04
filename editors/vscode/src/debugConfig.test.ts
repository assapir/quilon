// Unit tests for the pure debug-config helpers. No `vscode` dependency, so
// these run under plain Node: `pnpm test` compiles to out/ then `node --test`.

import assert from "node:assert/strict";
import * as path from "node:path";
import { test } from "node:test";
import {
  buildArgs,
  corelibSourceMap,
  firstNonEmptyLine,
  InFlightBuilds,
  splitCommand,
  tempBinaryPath,
  testBuildArgs,
  toLldbConfiguration,
} from "./debugConfig";

test("InFlightBuilds: first acquire wins, a concurrent one for the same key is refused", () => {
  const inFlight = new InFlightBuilds();
  assert.equal(inFlight.tryAcquire("/w/a.qn"), true);
  assert.equal(inFlight.tryAcquire("/w/a.qn"), false);
});

test("InFlightBuilds: a different key is independent", () => {
  const inFlight = new InFlightBuilds();
  assert.equal(inFlight.tryAcquire("/w/a.qn"), true);
  assert.equal(inFlight.tryAcquire("/w/b.qn"), true);
});

test("InFlightBuilds: release lets the key be acquired again (success/error path)", () => {
  const inFlight = new InFlightBuilds();
  inFlight.tryAcquire("/w/a.qn");
  inFlight.release("/w/a.qn");
  assert.equal(inFlight.tryAcquire("/w/a.qn"), true);
});

test("InFlightBuilds: release is idempotent and safe when nothing is held", () => {
  const inFlight = new InFlightBuilds();
  inFlight.release("/w/a.qn");
  assert.equal(inFlight.tryAcquire("/w/a.qn"), true);
});

test("firstNonEmptyLine: skips leading blank lines and trims", () => {
  assert.equal(firstNonEmptyLine("\n\n   error: boom  \nmore"), "error: boom");
});

test("firstNonEmptyLine: undefined when there is no content", () => {
  assert.equal(firstNonEmptyLine("\n   \n"), undefined);
});

test("splitCommand: bare `quilon` has no base args", () => {
  assert.deepEqual(splitCommand("quilon"), { exe: "quilon", baseArgs: [] });
});

test("splitCommand: `cargo run --` splits into exe + leading args", () => {
  assert.deepEqual(splitCommand("cargo run --"), { exe: "cargo", baseArgs: ["run", "--"] });
});

test("splitCommand: empty/whitespace falls back to quilon", () => {
  assert.deepEqual(splitCommand("   "), { exe: "quilon", baseArgs: [] });
});

test("buildArgs: emits a `build --debug <file> -o <out>` invocation", () => {
  assert.deepEqual(buildArgs([], "/w/app.qn", "/tmp/app"), [
    "build",
    "--debug",
    "/w/app.qn",
    "-o",
    "/tmp/app",
  ]);
});

test("buildArgs: preserves base args from the command setting", () => {
  assert.deepEqual(buildArgs(["run", "--"], "/w/app.qn", "/tmp/app"), [
    "run",
    "--",
    "build",
    "--debug",
    "/w/app.qn",
    "-o",
    "/tmp/app",
  ]);
});

test("testBuildArgs: a suite/case path becomes `--only <path> --binary <out>`", () => {
  assert.deepEqual(testBuildArgs([], "/w/suite.qn", "Suite/case", "/tmp/out"), [
    "test",
    "/w/suite.qn",
    "--only",
    "Suite/case",
    "--binary",
    "/tmp/out",
  ]);
});

test("testBuildArgs: no path builds the whole file, with no `--only`", () => {
  assert.deepEqual(testBuildArgs([], "/w/suite.qn", undefined, "/tmp/out"), [
    "test",
    "/w/suite.qn",
    "--binary",
    "/tmp/out",
  ]);
});

test("testBuildArgs: preserves base args from the command setting", () => {
  assert.deepEqual(testBuildArgs(["run", "--"], "/w/suite.qn", "Suite", "/tmp/out"), [
    "run",
    "--",
    "test",
    "/w/suite.qn",
    "--only",
    "Suite",
    "--binary",
    "/tmp/out",
  ]);
});

test("tempBinaryPath: strips .qn, embeds base name + pid + uniquifier, honors tmpDir", () => {
  const out = tempBinaryPath("/some/where/factorial.qn", "abc", "/mytmp");
  assert.equal(out, path.join("/mytmp", `quilon-debug-factorial-${process.pid}-abc`));
});

test("tempBinaryPath: is case-insensitive on the .qn extension", () => {
  const out = tempBinaryPath("/x/APP.QN", "1", "/t");
  assert.equal(path.basename(out), `quilon-debug-APP-${process.pid}-1`);
});

test("toLldbConfiguration: resolves to a CodeLLDB launch of the built binary", () => {
  const config = toLldbConfiguration({
    name: "Quilon: Debug current file",
    program: "/tmp/app",
    args: ["a", "b"],
    cwd: "/w",
  });
  assert.equal(config.type, "lldb");
  assert.equal(config.request, "launch");
  assert.equal(config.program, "/tmp/app");
  assert.deepEqual(config.args, ["a", "b"]);
  assert.equal(config.cwd, "/w");
  assert.equal(config.initCommands, undefined);
});

test("toLldbConfiguration: routes I/O to the shared Debug Console (no per-run terminal)", () => {
  const config = toLldbConfiguration({ name: "n", program: "/tmp/app" });
  assert.equal(config.terminal, "console");
});

test("toLldbConfiguration: imports the formatter when a path is given", () => {
  const config = toLldbConfiguration({
    name: "Quilon Debug",
    program: "/tmp/app",
    formatterPath: "/ext/formatters/quilon.py",
  });
  assert.deepEqual(config.initCommands, ['command script import "/ext/formatters/quilon.py"']);
});

test("toLldbConfiguration: defaults missing args and cwd", () => {
  const config = toLldbConfiguration({ name: "n", program: "/tmp/app" });
  assert.deepEqual(config.args, []);
  assert.equal(config.cwd, "${workspaceFolder}");
});

test("toLldbConfiguration: no sourceMap when corelibDir is missing", () => {
  const config = toLldbConfiguration({
    name: "n",
    program: "/tmp/app",
    sourceFile: "/w/examples/hello.qn",
  });
  assert.equal(config.sourceMap, undefined);
});

test("toLldbConfiguration: no sourceMap when sourceFile is missing", () => {
  const config = toLldbConfiguration({
    name: "n",
    program: "/tmp/app",
    corelibDir: "/home/user/.cache/quilon/corelib-0.10.0",
  });
  assert.equal(config.sourceMap, undefined);
});

test("toLldbConfiguration: adds the corelib sourceMap when both are given", () => {
  const config = toLldbConfiguration({
    name: "n",
    program: "/tmp/app",
    sourceFile: "/w/examples/hello.qn",
    corelibDir: "/home/user/.cache/quilon/corelib-0.10.0",
  });
  assert.deepEqual(config.sourceMap, {
    "/w/examples/corelib": "/home/user/.cache/quilon/corelib-0.10.0/corelib",
  });
});

// --- corelibSourceMap ---------------------------------------------------------
//
// The "from" half is the debugged FILE's own directory + `corelib` — where DWARF (per
// `dwarf_file_location` in `src/codegen/debug.rs`) attributes a corelib function's source,
// resolved against the compile unit's `DW_AT_comp_dir` — not the corelib module's own real
// location, which doesn't exist on disk at all.

test("corelibSourceMap: maps <source dir>/corelib to <corelibDir>/corelib", () => {
  const map = corelibSourceMap(
    "/w/examples/http_get.qn",
    "/home/user/.cache/quilon/corelib-0.10.0",
  );
  assert.deepEqual(map, {
    "/w/examples/corelib": "/home/user/.cache/quilon/corelib-0.10.0/corelib",
  });
});

test("corelibSourceMap: a different source directory maps its own corelib prefix", () => {
  const map = corelibSourceMap("/w/tests/suite.qn", "/home/user/.cache/quilon/corelib-0.10.0");
  assert.deepEqual(map, {
    "/w/tests/corelib": "/home/user/.cache/quilon/corelib-0.10.0/corelib",
  });
});
