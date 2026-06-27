// Unit tests for the entry-point detector. No `vscode` dependency, so these run
// under plain Node: `pnpm test` compiles to out/ then `node --test`.

import assert from "node:assert/strict";
import { test } from "node:test";
import { findEntryPoints } from "./entryPoints";

test("finds a single `^ =` entry point", () => {
  const text = ["^ = () -> Num => 42"].join("\n");
  assert.deepEqual(findEntryPoints(text), [{ line: 0, column: 0 }]);
});

test("finds a `^ ::` type-annotation form", () => {
  const text = ["^ :: () -> Num"].join("\n");
  assert.deepEqual(findEntryPoints(text), [{ line: 0, column: 0 }]);
});

test("tolerates no space between `^` and the operator", () => {
  assert.deepEqual(findEntryPoints("^= () -> Num => 0"), [{ line: 0, column: 0 }]);
});

test("returns nothing when there is no `^`", () => {
  const text = ["greet = (name :: Text) -> Text => name", "x = 1"].join("\n");
  assert.deepEqual(findEntryPoints(text), []);
});

test("matches `^` with leading whitespace and reports its column", () => {
  const text = ["    ^ = () -> Num => 0"].join("\n");
  assert.deepEqual(findEntryPoints(text), [{ line: 0, column: 4 }]);
});

test("does NOT match a `^` inside a line comment", () => {
  const text = ["~ here is a ^ = decoy in a comment", "x = 1"].join("\n");
  assert.deepEqual(findEntryPoints(text), []);
});

test("does NOT match a `^` inside a string", () => {
  const text = ['msg = "^ = not an entry point"'].join("\n");
  assert.deepEqual(findEntryPoints(text), []);
});

test("does NOT match a bare `^` used as an operator mid-expression", () => {
  // `^` is only an entry point when it leads the line and is followed by `=`/`::`.
  const text = ["y = a ^ b"].join("\n");
  assert.deepEqual(findEntryPoints(text), []);
});

test("finds multiple entry points across lines", () => {
  const text = ["^ = () -> Num => 1", "helper = () -> Num => 2", "  ^ :: () -> Num"].join("\n");
  assert.deepEqual(findEntryPoints(text), [
    { line: 0, column: 0 },
    { line: 2, column: 2 },
  ]);
});

test("tolerates CRLF line endings", () => {
  const text = "helper = () -> Num => 1\r\n^ = () -> Num => 0\r\n";
  assert.deepEqual(findEntryPoints(text), [{ line: 1, column: 0 }]);
});
