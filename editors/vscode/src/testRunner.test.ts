// Unit tests for the pure Test Explorer helpers. No `vscode` dependency, so these run
// under plain Node: `pnpm test` compiles to out/ then `node --test`.

import assert from "node:assert/strict";
import { test } from "node:test";
import { buildTestTree, parseReporterLine, testRunArgs, type TestItemInfo } from "./testRunner";

const range = { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } };

function item(path: string, name: string, kind: "suite" | "case"): TestItemInfo {
  return { path, name, kind, range };
}

test("buildTestTree: nests by `/`-prefix, a flat sibling list stays flat", () => {
  const tree = buildTestTree([item("a", "a", "suite"), item("b", "b", "suite")]);
  assert.equal(tree.length, 2);
  assert.deepEqual(
    tree.map((n) => n.path),
    ["a", "b"],
  );
  assert.equal(tree[0].children.length, 0);
});

test("buildTestTree: a case nests under its enclosing suite", () => {
  const tree = buildTestTree([item("s", "s", "suite"), item("s/c", "c", "case")]);
  assert.equal(tree.length, 1);
  assert.equal(tree[0].children.length, 1);
  assert.equal(tree[0].children[0].path, "s/c");
});

test("buildTestTree: matches the doc example — outer/first, outer/inner/second", () => {
  const items = [
    item("outer", "outer", "suite"),
    item("outer/first", "first", "case"),
    item("outer/inner", "inner", "suite"),
    item("outer/inner/second", "second", "case"),
  ];
  const tree = buildTestTree(items);
  assert.equal(tree.length, 1);
  const outer = tree[0];
  assert.deepEqual(
    outer.children.map((n) => n.path),
    ["outer/first", "outer/inner"],
  );
  const inner = outer.children[1];
  assert.equal(inner.children.length, 1);
  assert.equal(inner.children[0].path, "outer/inner/second");
});

test("buildTestTree: a suite reopens after a sibling closes (stack pops back to the right ancestor)", () => {
  const items = [
    item("s", "s", "suite"),
    item("s/a", "a", "suite"),
    item("s/a/x", "x", "case"),
    item("s/b", "b", "suite"),
    item("s/b/y", "y", "case"),
  ];
  const tree = buildTestTree(items);
  const s = tree[0];
  assert.deepEqual(
    s.children.map((n) => n.path),
    ["s/a", "s/b"],
  );
  assert.equal(s.children[0].children[0].path, "s/a/x");
  assert.equal(s.children[1].children[0].path, "s/b/y");
});

test("buildTestTree: empty input is an empty tree", () => {
  assert.deepEqual(buildTestTree([]), []);
});

test("testRunArgs: whole-file run has no `--only`", () => {
  assert.deepEqual(testRunArgs([], "/w/a.qn", []), ["test", "/w/a.qn", "--reporter", "json"]);
});

test("testRunArgs: one `--only` per selected path, preserves base args", () => {
  assert.deepEqual(testRunArgs(["run", "--"], "/w/a.qn", ["Suite/case"]), [
    "run",
    "--",
    "test",
    "/w/a.qn",
    "--reporter",
    "json",
    "--only",
    "Suite/case",
  ]);
});

test("testRunArgs: several selected paths repeat `--only`", () => {
  assert.deepEqual(testRunArgs([], "/w/a.qn", ["S/a", "S/b"]), [
    "test",
    "/w/a.qn",
    "--reporter",
    "json",
    "--only",
    "S/a",
    "--only",
    "S/b",
  ]);
});

test("parseReporterLine: a suite event", () => {
  assert.deepEqual(parseReporterLine('{"event":"suite","path":"Text","depth":0}'), {
    event: "suite",
    path: "Text",
    depth: 0,
  });
});

test("parseReporterLine: a passing case event", () => {
  assert.deepEqual(parseReporterLine('{"event":"case","path":"Text/trims","status":"pass"}'), {
    event: "case",
    path: "Text/trims",
    status: "pass",
  });
});

test("parseReporterLine: a failing case event carries message/file/line", () => {
  const line =
    '{"event":"case","path":"Text/splits","status":"fail","message":"expected 4, got 3","file":"tests/text.qn","line":9}';
  assert.deepEqual(parseReporterLine(line), {
    event: "case",
    path: "Text/splits",
    status: "fail",
    message: "expected 4, got 3",
    file: "tests/text.qn",
    line: 9,
  });
});

test("parseReporterLine: a summary event", () => {
  assert.deepEqual(parseReporterLine('{"event":"summary","passed":1,"failed":1}'), {
    event: "summary",
    passed: 1,
    failed: 1,
  });
});

test("parseReporterLine: a blank line is undefined, not an error", () => {
  assert.equal(parseReporterLine(""), undefined);
  assert.equal(parseReporterLine("   "), undefined);
});

test("parseReporterLine: malformed or truncated JSON is undefined, not a throw", () => {
  assert.equal(parseReporterLine("{not json"), undefined);
  assert.equal(parseReporterLine('{"event":"case","path":"x"'), undefined);
});

test("parseReporterLine: an unrecognized event name is undefined", () => {
  assert.equal(parseReporterLine('{"event":"mystery"}'), undefined);
});

test("parseReporterLine: a case event missing its required fail fields is undefined", () => {
  assert.equal(parseReporterLine('{"event":"case","path":"x","status":"fail"}'), undefined);
});
