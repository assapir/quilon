// A small, faithful re-implementation of how a TextMate grammar tokenizes a
// single line, used to test `syntaxes/quilon.tmLanguage.json` without pulling in
// the (native) `vscode-textmate` + `vscode-oniguruma` engine as a dependency.
//
// It reproduces the two behaviours this grammar's correctness hinges on:
//
// - At each position TextMate scans the *ordered* list of patterns and applies
//   the FIRST one that matches there — ties at the same start position are
//   decided by list order, NOT by match length. That is exactly why every
//   multi-character operator (`=>`, `->`, `:=`, `<-`, `==`, `!=`, `<=`, `>=`,
//   `&&`, `||`, `::`) must be listed before the single-character operator
//   rules: otherwise a rule for the first character would win and split the
//   operator into two tokens. The same tie-break is why a string's `` (a
//   literal-backtick escape) must be listed before its single-backtick
//   interpolation-hole rule.
// - A `begin`/`end` rule's own scope applies to its whole span by default —
//   including any content its `patterns` don't otherwise claim — with a
//   nested `begin`/`end` (an interpolation hole inside a string, a nested
//   string inside that) recursing the same way, so a hole gets its own scope
//   and a `"` inside one cannot close the enclosing string. `end` competes
//   with `patterns` for the earliest match, same as TextMate's default
//   `applyEndPatternLast: false`: a tie is won by `end`.
//
// Supported subset (all this grammar uses): `match` rules, single-line
// `begin`/`end` rules (nestable, with `beginCaptures`/`endCaptures` and
// `patterns`), and `#name` references into `repository` — including a named
// rule re-including one of its own ancestors (an interpolation hole includes
// `#expressions`, which includes `#strings`, the hole's own enclosing rule).
// A `begin`/`end` rule's `patterns` are resolved lazily, on first use, with a
// fresh include-cycle guard: resolving them can only happen after a `begin`
// has actually matched some text, so unlike a flat (no `begin`/`end` in
// between) named-group cycle, it can't recurse while the grammar itself is
// still being built, and reusing the *same* rule deeper in the source (the
// hole again, inside its own nested string) is genuine recursion to support,
// not a cycle to reject. This module is deliberately free of any `vscode`
// import so it runs under plain Node (`node:test`), like `diagnostics.ts`.
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
  /** Per-capture-group scope names (keyed by group index as a string; index `0` is the whole match). */
  readonly captures?: Readonly<Record<string, { readonly name: string }>>;
  readonly beginCaptures?: Readonly<Record<string, { readonly name: string }>>;
  readonly endCaptures?: Readonly<Record<string, { readonly name: string }>>;
}

interface RawGrammar {
  readonly patterns: readonly RawRule[];
  readonly repository: Readonly<Record<string, RawRule>>;
}

/** One tokenized slice of a line: its text and the scope name applied to it. */
export interface Token {
  readonly text: string;
  /** The grammar `name` scope, or `undefined` for unscoped (plain, top-level) text. */
  readonly scope: string | undefined;
}

/** Capture-group index → scope name. */
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
      readonly beginCaptures: Captures;
      readonly end: RegExp;
      readonly endCaptures: Captures;
      /** Lazily resolved and memoized — see the header comment on why a fresh cycle guard. */
      readonly children: () => readonly Rule[];
    };

export class Grammar {
  private readonly rootRules: readonly Rule[];

  private constructor(grammar: RawGrammar) {
    this.rootRules = resolve(grammar.patterns, grammar.repository, new Set());
  }

  /** Load and compile a tmLanguage JSON grammar from disk. */
  static fromFile(path: string): Grammar {
    return new Grammar(JSON.parse(readFileSync(path, "utf8")) as RawGrammar);
  }

  /**
   * Tokenize a single line. Returns the slices in order; their concatenated
   * `text` reproduces the input exactly. Plain (unmatched) top-level text gets
   * an `undefined` scope; unmatched text inside a `begin`/`end` span gets that
   * rule's own scope (mirroring how TextMate applies a block's `name` to its
   * whole span).
   */
  tokenizeLine(line: string): Token[] {
    const tokens: Token[] = [];
    scan(line, 0, this.rootRules, undefined, undefined, tokens);
    return tokens;
  }
}

/** An enclosing `begin`/`end` rule's own closing pattern, while scanning its content. */
interface EndSpec {
  readonly re: RegExp;
  readonly captures: Captures;
}

/** The earliest-matching candidate at a scan position: `end`, or one pattern rule. */
type Best =
  | { readonly kind: "end"; readonly m: RegExpExecArray; readonly captures: Captures }
  | { readonly kind: "rule"; readonly m: RegExpExecArray; readonly rule: Rule };

/**
 * Scan `line` from `pos` to the end (or to where `end` closes), applying
 * `rules`. `enclosing` is the scope given to any run of characters `rules`
 * doesn't otherwise claim — `undefined` at the top level, an enclosing
 * `begin`/`end` rule's own `name` while scanning inside it. Returns the
 * position the scan stopped at: the line length, or just past a matched `end`.
 */
function scan(
  line: string,
  pos: number,
  rules: readonly Rule[],
  end: EndSpec | undefined,
  enclosing: string | undefined,
  tokens: Token[],
): number {
  let plainStart = pos;
  const flushPlain = (upTo: number): void => {
    if (upTo > plainStart) {
      tokens.push({ text: line.slice(plainStart, upTo), scope: enclosing });
    }
  };

  while (pos < line.length) {
    let best: Best | undefined;

    if (end) {
      const m = earliestMatchFrom(end.re, line, pos);
      if (m) {
        best = { kind: "end", m, captures: end.captures };
      }
    }
    // A strictly-earlier pattern match overrides `end`; an equal start keeps
    // `end` (TextMate's default `applyEndPatternLast: false`). Among the
    // patterns themselves, an equal start keeps the earlier-listed rule.
    for (const rule of rules) {
      const re = rule.kind === "match" ? rule.re : rule.begin;
      const m = earliestMatchFrom(re, line, pos);
      if (m && (!best || m.index < best.m.index)) {
        best = { kind: "rule", m, rule };
      }
    }

    if (!best) {
      break;
    }
    flushPlain(best.m.index);

    if (best.kind === "end") {
      tokens.push(...capturedTokens(best.m, best.captures, enclosing));
      return best.m.index + best.m[0].length;
    }

    const { rule, m } = best;
    if (rule.kind === "match") {
      tokens.push(...capturedTokens(m, rule.captures, rule.name));
      pos = m.index + m[0].length;
    } else {
      tokens.push(...capturedTokens(m, rule.beginCaptures, rule.name));
      pos = scan(
        line,
        m.index + m[0].length,
        rule.children(),
        { re: rule.end, captures: rule.endCaptures },
        rule.name,
        tokens,
      );
    }
    plainStart = pos;
  }

  flushPlain(line.length);
  return line.length;
}

/** Flatten a pattern list into leaf rules, resolving `#include` against the repo. */
function resolve(
  patterns: readonly RawRule[],
  repo: Readonly<Record<string, RawRule>>,
  seen: ReadonlySet<string>,
): Rule[] {
  const out: Rule[] = [];
  for (const p of patterns) {
    if (p.include) {
      const key = p.include.replace(/^#/, "");
      if (seen.has(key)) {
        continue; // guard against a flat (no begin/end in between) include cycle
      }
      const target = repo[key];
      if (!target) {
        continue;
      }
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
      {
        kind: "match",
        name: rule.name,
        re: sticky(rule.match),
        captures: buildCaptures(rule.captures),
      },
    ];
  }
  if (typeof rule.begin === "string") {
    const rawChildren = rule.patterns ?? [];
    let children: readonly Rule[] | undefined;
    return [
      {
        kind: "beginEnd",
        name: rule.name,
        begin: sticky(rule.begin),
        beginCaptures: buildCaptures(rule.beginCaptures),
        end: sticky(rule.end ?? "$"),
        endCaptures: buildCaptures(rule.endCaptures),
        // A fresh cycle guard, not the inherited `seen`: this resolves lazily
        // (on first use, after `begin` already matched), so re-including an
        // ancestor here — a hole re-including `#strings` — is real recursion
        // through the source text, not the eager flat cycle `seen` guards
        // against. Reusing `seen` here would permanently mark e.g. "strings"
        // as visited and silently drop it from every hole ever after.
        children: () => (children ??= resolve(rawChildren, repo, new Set())),
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
 * earliest start cleanly and never silently skips ahead), `d` exposes each
 * group's span so a `captures` map can be applied as sub-tokens, and `u` turns
 * on the `\p{...}` Unicode property escapes the grammar's own identifier rules
 * use (real Oniguruma, which VS Code runs on, supports those natively with no
 * flag).
 */
function sticky(source: string): RegExp {
  return new RegExp(source, "yud");
}

/** Read a `captures`/`beginCaptures`/`endCaptures` object into an index→scope map. */
function buildCaptures(captures: RawRule["captures"]): Captures {
  const map = new Map<number, string>();
  if (captures) {
    for (const [index, value] of Object.entries(captures)) {
      map.set(Number(index), value.name);
    }
  }
  return map;
}

/**
 * Earliest match of a sticky regex at or after `from`, or undefined. Returns
 * the full exec result (capture group spans included) so a caller never has
 * to re-run the regex to recover them.
 */
function earliestMatchFrom(re: RegExp, line: string, from: number): RegExpExecArray | undefined {
  for (let at = from; at <= line.length; at++) {
    re.lastIndex = at;
    const m = re.exec(line);
    // A zero-width match (e.g. `end: "$"`) is only useful at end-of-line —
    // elsewhere it would loop forever without ever consuming a character.
    if (m && (m[0].length > 0 || at === line.length)) {
      return m;
    }
  }
  return undefined;
}

/**
 * Tokenize one already-found match `m` (a `match` rule, or a `begin`/`end`
 * rule's own begin/end/text). With no captures it is a single token scoped to
 * `defaultName`; with captures, `defaultName` fills any span no capture
 * covers and each capture group layers its scope on top (group `0` is the
 * whole match; a later/inner group overrides an earlier/outer one where they
 * overlap), matching how a theme colors a captured match.
 */
function capturedTokens(
  m: RegExpExecArray,
  captures: Captures,
  defaultName: string | undefined,
): Token[] {
  const whole = m[0];
  if (whole.length === 0) {
    return [];
  }
  if (captures.size === 0) {
    return [{ text: whole, scope: defaultName }];
  }

  const start = m.index;
  const scopes: (string | undefined)[] = Array.from({ length: whole.length }, () => defaultName);
  const indices = m.indices;
  if (indices) {
    for (let g = 0; g < indices.length; g++) {
      const span = indices[g];
      const scope = captures.get(g);
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
