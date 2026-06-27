// Unit tests for the diagnostic-output parser. No `vscode` dependency, so these
// run under plain Node: `npm test` compiles to out/ then `node --test`.

import assert from "node:assert/strict";
import { test } from "node:test";
import { parseDiagnostics } from "./diagnostics";

test("parses a single diagnostic with a caret span", () => {
  const output = [
    "🔍 Checking: examples/type_error.ql",
    "examples/type_error.ql:5:5: error: Unexpected token: TypeAnnotation",
    "  |",
    '5 |   x :: Num = "not a number"',
    "  |     ^^",
  ].join("\n");

  const diagnostics = parseDiagnostics(output);
  assert.equal(diagnostics.length, 1);
  assert.deepEqual(diagnostics[0], {
    file: "examples/type_error.ql",
    line: 5,
    column: 5,
    message: "Unexpected token: TypeAnnotation",
    span: 2,
  });
});

test("ignores status banners and source-context lines", () => {
  const output = ["🔍 Checking: foo.ql", "📋 some other noise", "✅ nothing here"].join("\n");
  assert.deepEqual(parseDiagnostics(output), []);
});

test("header without a caret block has no span", () => {
  const output = "foo.ql:1:1: error: something broke";
  const diagnostics = parseDiagnostics(output);
  assert.equal(diagnostics.length, 1);
  assert.equal(diagnostics[0].span, undefined);
  assert.equal(diagnostics[0].message, "something broke");
});

test("parses multiple diagnostics, each with its own span", () => {
  const output = [
    "a.ql:1:1: error: first",
    "  |",
    "1 | bad one",
    "  | ^^^",
    "a.ql:2:3: error: second",
    "  |",
    "2 |   bad two",
    "  |   ^^^^^^^",
  ].join("\n");

  const diagnostics = parseDiagnostics(output);
  assert.equal(diagnostics.length, 2);
  assert.equal(diagnostics[0].message, "first");
  assert.equal(diagnostics[0].span, 3);
  assert.equal(diagnostics[1].line, 2);
  assert.equal(diagnostics[1].column, 3);
  assert.equal(diagnostics[1].span, 7);
});

test("a header's caret window does not bleed into the next diagnostic", () => {
  // Back-to-back headers with no context block: neither gets a span.
  const output = ["x.ql:1:1: error: one", "x.ql:2:2: error: two"].join("\n");
  const diagnostics = parseDiagnostics(output);
  assert.equal(diagnostics.length, 2);
  assert.equal(diagnostics[0].span, undefined);
  assert.equal(diagnostics[1].span, undefined);
});

test("handles a Windows-style path containing a drive colon", () => {
  const output = "C:\\src\\main.ql:3:7: error: nope";
  const diagnostics = parseDiagnostics(output);
  assert.equal(diagnostics.length, 1);
  assert.equal(diagnostics[0].file, "C:\\src\\main.ql");
  assert.equal(diagnostics[0].line, 3);
  assert.equal(diagnostics[0].column, 7);
});

test("tolerates CRLF line endings", () => {
  const output = "f.ql:1:1: error: boom\r\n  |\r\n1 | x\r\n  | ^\r\n";
  const diagnostics = parseDiagnostics(output);
  assert.equal(diagnostics.length, 1);
  assert.equal(diagnostics[0].message, "boom");
  assert.equal(diagnostics[0].span, 1);
});
