// Tokenization tests for the Quilon TextMate grammar
// (`syntaxes/quilon.tmLanguage.json`).
//
// These guard the bug where a multi-character operator (`=>`, `->`, …) was at
// risk of being highlighted as TWO tokens (its first char in one scope, the
// rest in another). TextMate applies, at each position, the FIRST pattern in
// the list that matches — ties at the same start are decided by list order, not
// length — so the fix is purely about ordering the multi-char operator rules
// before the single-char ones. We assert each operator yields exactly one token
// with one scope.
//
// We tokenize with a small faithful re-implementation of that algorithm
// (`./grammar`) rather than the native `vscode-textmate` engine, to keep the
// tests dependency-free and runnable under plain `node --test`.

import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";
import { Grammar, type Token } from "./grammar";

const grammar = Grammar.fromFile(join(__dirname, "..", "syntaxes", "quilon.tmLanguage.json"));

/** All scoped (non-plain) tokens of a line, in order. */
function scopedTokens(line: string): Token[] {
  return grammar.tokenizeLine(line).filter((t) => t.scope !== undefined);
}

/** Find the single token whose text is exactly `op`; fail if 0 or >1. */
function uniqueToken(line: string, op: string): Token {
  const matches = grammar.tokenizeLine(line).filter((t) => t.text === op);
  assert.equal(
    matches.length,
    1,
    `expected exactly one token with text ${JSON.stringify(op)} in ${JSON.stringify(line)}, ` +
      `got ${JSON.stringify(grammar.tokenizeLine(line))}`,
  );
  return matches[0];
}

// Every multi-character operator and the single scope it must carry. Each is
// exercised in both a spaced (`a OP b`) and a tight (`aOPb`) line, generated
// from the operator — the tight form is where a naive grammar is most likely to
// split the operator into its first character + the rest, and in `a OP b` the
// operator's first character has no legitimate standalone twin, so a standalone
// first-char token would be the split-operator regression.
const MULTI_CHAR_OPERATORS: ReadonlyArray<readonly [op: string, scope: string]> = [
  ["=>", "keyword.operator.arrow.body.quilon"],
  ["->", "keyword.operator.arrow.return.quilon"],
  [":=", "keyword.operator.assignment.mutable.quilon"],
  ["|>", "keyword.operator.pipeline.quilon"],
  ["<-", "keyword.operator.arrow.iterate.quilon"],
  ["::", "keyword.operator.type-annotation.quilon"],
  ["==", "keyword.operator.comparison.quilon"],
  ["!=", "keyword.operator.comparison.quilon"],
  ["<=", "keyword.operator.comparison.quilon"],
  [">=", "keyword.operator.comparison.quilon"],
  ["&&", "keyword.operator.logical.quilon"],
  ["||", "keyword.operator.logical.quilon"],
];

for (const [op, scope] of MULTI_CHAR_OPERATORS) {
  const spaced = `a ${op} b`;
  const tight = `a${op}b`;

  test(`multi-char operator ${op} is one token with scope ${scope}`, () => {
    for (const line of [spaced, tight]) {
      const token = uniqueToken(line, op);
      assert.equal(token.text, op);
      assert.equal(token.scope, scope);
    }
  });

  test(`multi-char operator ${op} is not split into its first character`, () => {
    // The regression: the first char would appear as its own scoped token.
    const firstChar = op[0];
    const standalone = grammar
      .tokenizeLine(spaced)
      .filter((t) => t.scope !== undefined && t.text === firstChar);
    assert.equal(
      standalone.length,
      0,
      `${op} leaked a standalone ${JSON.stringify(firstChar)} token: ` +
        JSON.stringify(grammar.tokenizeLine(spaced)),
    );
  });
}

// `<<` / `>>` are module markers (import / export), handled before the operator
// rules — assert they are still single tokens too.
test("<< import marker is a single token", () => {
  const token = uniqueToken("<< core.io", "<<");
  assert.equal(token.scope, "keyword.control.import.quilon");
});

test(">> export marker is a single token", () => {
  const token = uniqueToken(">> add = (a, b) => a + b", ">>");
  assert.equal(token.scope, "keyword.control.export.quilon");
});

test("adjacent operators on one line each stay a single token", () => {
  // `a==b!=c<=d>=e` — four two-char comparisons back to back, no spaces.
  const tokens = scopedTokens("a==b!=c<=d>=e");
  const ops = tokens.filter((t) => t.scope === "keyword.operator.comparison.quilon");
  assert.deepEqual(
    ops.map((t) => t.text),
    ["==", "!=", "<=", ">="],
  );
});

test("a representative lambda + arrow-type line highlights each operator once", () => {
  // `double = (x :: Num) -> Num => x * 2`
  const line = "double = (x :: Num) -> Num => x * 2";
  const tokens = scopedTokens(line);
  const find = (text: string) => tokens.filter((t) => t.text === text);

  assert.equal(find("::").length, 1, ":: should be one token");
  assert.equal(find("->").length, 1, "-> should be one token");
  assert.equal(find("=>").length, 1, "=> should be one token");
  assert.equal(find("=").length, 1, "the single = should be one token");
  // No stray single-char fragments from the multi-char operators.
  assert.equal(find(">").length, 0, "no bare > fragment");
  assert.equal(find(":").length, 0, "no bare : fragment");
});

// --- Regression guards for things the fix must NOT disturb -------------------

test("single < and > stay comparison operators (and block delimiters)", () => {
  const lt = uniqueToken("a < b", "<");
  assert.equal(lt.scope, "keyword.operator.comparison.quilon");
  const gt = uniqueToken("a > b", ">");
  assert.equal(gt.scope, "keyword.operator.comparison.quilon");
});

test("single = stays an immutable-binding operator", () => {
  const token = uniqueToken("x = 42", "=");
  assert.equal(token.scope, "keyword.operator.assignment.quilon");
});

test("$ (unit) keeps its builtin-type scope in both type and value position", () => {
  // `f = () -> $ => $`: the `$` return type and the `$` value are both scoped,
  // and the surrounding `->` / `=>` are each still a single operator token.
  const tokens = grammar.tokenizeLine("f = () -> $ => $");
  const units = tokens.filter((t) => t.text === "$");
  assert.equal(units.length, 2);
  for (const u of units) {
    assert.equal(u.scope, "support.type.builtin.unit.quilon");
  }
  assert.equal(tokens.filter((t) => t.text === "->").length, 1);
  assert.equal(tokens.filter((t) => t.text === "=>").length, 1);
});

test("~ comment swallows operators to end of line", () => {
  const tokens = grammar.tokenizeLine("~ a note => not an operator");
  // The whole comment is one scoped token; no operator scope leaks out of it.
  const operatorTokens = tokens.filter((t) => t.scope?.startsWith("keyword.operator"));
  assert.equal(operatorTokens.length, 0);
  assert.ok(
    tokens.some((t) => t.scope === "comment.line.tilde.quilon"),
    "expected a comment token",
  );
});

test("string contents are not tokenized as operators", () => {
  const tokens = grammar.tokenizeLine('s = "a => b"');
  const operatorInString = tokens.filter(
    (t) => t.text === "=>" && t.scope?.startsWith("keyword.operator"),
  );
  assert.equal(operatorInString.length, 0, "=> inside a string must not be an operator");
});

test("numbers still tokenize", () => {
  const token = uniqueToken("x = 3.14", "3.14");
  assert.equal(token.scope, "constant.numeric.quilon");
});
