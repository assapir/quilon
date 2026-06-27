// A small, faithful re-implementation of how a TextMate grammar tokenizes a
// single line, used to test `syntaxes/quilon.tmLanguage.json` without pulling in
// the (native) `vscode-textmate` + `vscode-oniguruma` engine as a dependency.
//
// It reproduces the one behaviour this grammar's correctness hinges on: at each
// position TextMate scans the *ordered* list of patterns and applies the FIRST
// one that matches there — ties at the same start position are decided by list
// order, NOT by match length. That is exactly why every multi-character operator
// (`=>`, `->`, `:=`, `|>`, `<-`, `==`, `!=`, `<=`, `>=`, `&&`, `||`, `::`) must
// be listed before the single-character operator rules: otherwise a rule for the
// first character would win and split the operator into two tokens.
//
// Supported subset (all this grammar uses): `match` rules, `begin`/`end` rules
// (single line — enough for `~` comments and `"…"` strings), `#include`
// references into `repository`, and a single top-level `name`. This module is
// deliberately free of any `vscode` import so it runs under plain Node
// (`node:test`), like `diagnostics.ts`.
//
// Fidelity caveat: the grammar's regexes are run as JavaScript regexes here, not
// Oniguruma (which the real engine uses). The grammar's patterns stay within the
// common subset, so keep new patterns there too — an Oniguruma-only construct
// (e.g. `\G`, possessive quantifiers) would pass these tests yet differ in the
// editor.

import { readFileSync } from "node:fs";

/** A `name`+`match` (or `begin`/`end`) leaf rule, or an `include` reference. */
interface RawRule {
  readonly name?: string;
  readonly match?: string;
  readonly begin?: string;
  readonly end?: string;
  readonly include?: string;
  readonly patterns?: readonly RawRule[];
  /** Per-capture-group scope names (keyed by group index as a string). */
  readonly captures?: Readonly<Record<string, { readonly name: string }>>;
}

interface RawGrammar {
  readonly patterns: readonly RawRule[];
  readonly repository: Readonly<Record<string, RawRule>>;
}

/** One tokenized slice of a line: its text and the scope name applied to it. */
export interface Token {
  readonly text: string;
  /** The grammar `name` scope, or `undefined` for unscoped (plain) text. */
  readonly scope: string | undefined;
}

/** Capture-group index → scope name (for a `match` rule's `captures`). */
type Captures = ReadonlyMap<number, string>;

/** A leaf rule that actually matches text (an `include` has been resolved away). */
type Rule =
  | {
      readonly kind: "match";
      readonly name?: string;
      readonly re: RegExp;
      readonly captures: Captures;
    }
  | {
      readonly kind: "beginEnd";
      readonly name?: string;
      readonly begin: RegExp;
      /** Global regex (compiled once) for locating where the span closes. */
      readonly end: RegExp;
    };

export class Grammar {
  private readonly rootRules: readonly Rule[];

  private constructor(grammar: RawGrammar) {
    this.rootRules = resolve(grammar.patterns, grammar.repository);
  }

  /** Load and compile a tmLanguage JSON grammar from disk. */
  static fromFile(path: string): Grammar {
    return new Grammar(JSON.parse(readFileSync(path, "utf8")) as RawGrammar);
  }

  /**
   * Tokenize a single line. Returns the slices in order; their concatenated
   * `text` reproduces the input exactly. Plain (unmatched) runs get an
   * `undefined` scope.
   */
  tokenizeLine(line: string): Token[] {
    const tokens: Token[] = [];
    let pos = 0;
    let plainStart = 0;

    const flushPlain = (upTo: number): void => {
      if (upTo > plainStart) {
        tokens.push({ text: line.slice(plainStart, upTo), scope: undefined });
      }
    };

    while (pos < line.length) {
      const hit = firstMatch(this.rootRules, line, pos);
      if (!hit) {
        break;
      }
      flushPlain(hit.start);

      if (hit.rule.kind === "match") {
        tokens.push(...matchTokens(hit.rule, line, hit.start));
        pos = hit.end;
      } else {
        // begin/end: consume from `begin` through the first `end` on this line
        // (or to end-of-line if `end` is `$`/absent), as a single scoped token.
        const innerEnd = findEnd(hit.rule, line, hit.end);
        tokens.push({ text: line.slice(hit.start, innerEnd), scope: hit.rule.name });
        pos = innerEnd;
      }
      plainStart = pos;
    }

    flushPlain(line.length);
    return tokens;
  }
}

/** Flatten a pattern list into leaf rules, resolving `#include` against the repo. */
function resolve(
  patterns: readonly RawRule[],
  repo: Readonly<Record<string, RawRule>>,
  seen: ReadonlySet<string> = new Set(),
): Rule[] {
  const out: Rule[] = [];
  for (const p of patterns) {
    if (p.include) {
      const key = p.include.replace(/^#/, "");
      if (seen.has(key)) {
        continue; // guard against include cycles
      }
      const target = repo[key];
      if (!target) {
        continue;
      }
      // `compile` already dispatches on match / begin / bare-patterns, so the
      // include target goes through the same path as an inline rule.
      out.push(...compile(target, repo, new Set(seen).add(key)));
    } else {
      out.push(...compile(p, repo, seen));
    }
  }
  return out;
}

/** Turn a single leaf rule into its compiled form(s). */
function compile(
  rule: RawRule,
  repo: Readonly<Record<string, RawRule>>,
  seen: ReadonlySet<string>,
): Rule[] {
  if (typeof rule.match === "string") {
    return [
      { kind: "match", name: rule.name, re: sticky(rule.match), captures: buildCaptures(rule) },
    ];
  }
  if (typeof rule.begin === "string") {
    // The whole begin…end span is emitted as one scoped token (enough for `~`
    // comments and `"…"` strings), so any inner `patterns` are intentionally not
    // sub-scoped here — they don't affect the operator-tokenization this tests.
    return [
      {
        kind: "beginEnd",
        name: rule.name,
        begin: sticky(rule.begin),
        end: new RegExp(rule.end ?? "$", "g"),
      },
    ];
  }
  // A bare `{ patterns: [...] }` group (no match/begin): inline its children.
  if (rule.patterns) {
    return resolve(rule.patterns, repo, seen);
  }
  return [];
}

/**
 * Compile a TextMate regex as a JS *sticky* regex with capture indices: `y`
 * anchors a match to `lastIndex` (so probing position-by-position finds the
 * earliest start cleanly and never silently skips ahead), and `d` exposes each
 * group's span so a `match` rule's `captures` can be applied as sub-tokens.
 */
function sticky(source: string): RegExp {
  return new RegExp(source, "yd");
}

/** Read a rule's `captures` into an index→scope map. */
function buildCaptures(rule: RawRule): Captures {
  const map = new Map<number, string>();
  if (rule.captures) {
    for (const [index, value] of Object.entries(rule.captures)) {
      map.set(Number(index), value.name);
    }
  }
  return map;
}

interface Hit {
  readonly rule: Rule;
  readonly start: number;
  readonly end: number;
}

/**
 * Find the winning rule at-or-after `from`: the leftmost match across all rules,
 * ties at the same start broken by list order (the first rule wins) — TextMate's
 * exact rule. This list-order tiebreak is what makes operator ordering matter.
 */
function firstMatch(rules: readonly Rule[], line: string, from: number): Hit | undefined {
  let best: Hit | undefined;
  for (const rule of rules) {
    const re = rule.kind === "match" ? rule.re : rule.begin;
    const start = earliestMatchFrom(re, line, from);
    if (!start) {
      continue;
    }
    // Strictly-earlier start wins; an equal start keeps the earlier-listed rule.
    if (!best || start.index < best.start) {
      best = { rule, start: start.index, end: start.index + start.length };
    }
  }
  return best;
}

/** Earliest match of a sticky regex at or after `from`, or undefined. */
function earliestMatchFrom(
  re: RegExp,
  line: string,
  from: number,
): { index: number; length: number } | undefined {
  for (let at = from; at <= line.length; at++) {
    re.lastIndex = at;
    const m = re.exec(line);
    if (m && m[0].length > 0) {
      return { index: m.index, length: m[0].length };
    }
  }
  return undefined;
}

/**
 * Emit the token(s) for a `match` rule at `start`. With no `captures` it is a
 * single token scoped to the rule `name`; with `captures` the whole match takes
 * `name` and each capture group layers its scope onto its sub-span (a later/
 * inner group overrides an outer one for the characters it covers), matching how
 * a theme colors a captured match.
 */
function matchTokens(rule: Extract<Rule, { kind: "match" }>, line: string, start: number): Token[] {
  rule.re.lastIndex = start;
  const m = rule.re.exec(line);
  if (!m) {
    return [];
  }
  const whole = m[0];
  if (rule.captures.size === 0) {
    return [{ text: whole, scope: rule.name }];
  }

  // Per-character scope: start everyone at the rule name, then stamp each
  // capture group's span. Iterating groups by ascending index means a
  // higher-indexed (inner) capture overrides a lower one where they overlap.
  const scopes: (string | undefined)[] = Array.from({ length: whole.length }, () => rule.name);
  const indices = m.indices;
  if (indices) {
    for (let g = 1; g < indices.length; g++) {
      const span = indices[g];
      const scope = rule.captures.get(g);
      if (!span || scope === undefined) {
        continue;
      }
      for (let i = span[0] - start; i < span[1] - start; i++) {
        scopes[i] = scope;
      }
    }
  }

  // Coalesce runs of identical scope into tokens.
  const tokens: Token[] = [];
  let runStart = 0;
  for (let i = 1; i <= whole.length; i++) {
    if (i === whole.length || scopes[i] !== scopes[runStart]) {
      tokens.push({ text: whole.slice(runStart, i), scope: scopes[runStart] });
      runStart = i;
    }
  }
  return tokens;
}

/** For a begin/end rule, find where its `end` closes on this line. */
function findEnd(rule: Extract<Rule, { kind: "beginEnd" }>, line: string, from: number): number {
  rule.end.lastIndex = from;
  const m = rule.end.exec(line);
  if (!m) {
    return line.length;
  }
  // `$` matches with zero width at end-of-line: the comment runs to the line end.
  return m.index + (m[0].length > 0 ? m[0].length : line.length - m.index);
}
