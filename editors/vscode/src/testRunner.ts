// Pure helpers behind the Test Explorer: the `quilon test` argv it spawns, parsing the
// `--reporter json` event stream back, and nesting the language server's flat
// `quilon/testItems` list into a tree. Kept free of any `vscode` import so these run under
// plain Node and are unit-testable; `testExplorer.ts` wires them to the editor.
//
// The path every node here carries — the names from the outermost `describe` down, joined
// by `/` — is exactly what `quilon test --only` expects (see
// `docs/corelib/test/README.md#paths`); it is also the language server's `quilon/testItems`
// response and code-lens argument, so all three speak the same vocabulary.

/** A protocol position: zero-based line, UTF-16 column. */
export interface ItemPosition {
  line: number;
  character: number;
}

/** A protocol range, as `quilon/testItems` and `textDocument/codeLens` both carry it. */
export interface ItemRange {
  start: ItemPosition;
  end: ItemPosition;
}

/** One entry from the language server's `quilon/testItems` response. */
export interface TestItemInfo {
  path: string;
  name: string;
  kind: "suite" | "case";
  range: ItemRange;
}

/** A `TestItemInfo` nested under its enclosing suites. */
export interface TestNode extends TestItemInfo {
  children: TestNode[];
}

/**
 * Nest a flat, document-order `quilon/testItems` list into a tree, by path: an item whose
 * path has another's as a `/`-prefix is that item's descendant. The list is already in
 * document order (a suite before what it encloses), so a stack tracking the currently open
 * ancestors is enough — no sorting or path parsing needed beyond the prefix check.
 */
export function buildTestTree(items: readonly TestItemInfo[]): TestNode[] {
  const roots: TestNode[] = [];
  const openAncestors: TestNode[] = [];
  for (const item of items) {
    const node: TestNode = { ...item, children: [] };
    while (openAncestors.length > 0 && !isUnder(openAncestors[openAncestors.length - 1], node)) {
      openAncestors.pop();
    }
    const parent = openAncestors[openAncestors.length - 1];
    (parent?.children ?? roots).push(node);
    openAncestors.push(node);
  }
  return roots;
}

function isUnder(ancestor: TestNode, node: TestItemInfo): boolean {
  return node.path.startsWith(`${ancestor.path}/`);
}

/**
 * The `quilon test` argv for running `file`, restricted to `only` (the selected suite/case
 * paths; empty runs the whole file), reporting one JSON event per line so the run can be
 * parsed back rather than shown as text.
 */
export function testRunArgs(
  baseArgs: readonly string[],
  file: string,
  only: readonly string[],
): string[] {
  const args = [...baseArgs, "test", file, "--reporter", "json"];
  for (const path of only) {
    args.push("--only", path);
  }
  return args;
}

/** One `--reporter json` event, [the schema `quilon test` documents](../../../docs/corelib/test/README.md#the-json-events). */
export type ReporterEvent =
  | { event: "suite"; path: string; depth: number }
  | { event: "case"; path: string; status: "pass" }
  | { event: "case"; path: string; status: "fail"; message: string; file: string; line: number }
  | { event: "summary"; passed: number; failed: number };

/**
 * Parse one line of `--reporter json` output into its event, or `undefined` for a blank
 * line or one that isn't a recognized event — stdout under this reporter carries nothing
 * else, but a truncated final line (a still-filling buffer) must not throw.
 */
export function parseReporterLine(line: string): ReporterEvent | undefined {
  const trimmed = line.trim();
  if (trimmed.length === 0) {
    return undefined;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    return undefined;
  }
  return asReporterEvent(parsed);
}

function asReporterEvent(value: unknown): ReporterEvent | undefined {
  if (typeof value !== "object" || value === null || !("event" in value)) {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  switch (record["event"]) {
    case "suite":
      return typeof record["path"] === "string" && typeof record["depth"] === "number"
        ? { event: "suite", path: record["path"], depth: record["depth"] }
        : undefined;
    case "case":
      return asCaseEvent(record);
    case "summary":
      return typeof record["passed"] === "number" && typeof record["failed"] === "number"
        ? { event: "summary", passed: record["passed"], failed: record["failed"] }
        : undefined;
    default:
      return undefined;
  }
}

function asCaseEvent(record: Record<string, unknown>): ReporterEvent | undefined {
  if (typeof record["path"] !== "string") {
    return undefined;
  }
  if (record["status"] === "pass") {
    return { event: "case", path: record["path"], status: "pass" };
  }
  if (
    record["status"] === "fail" &&
    typeof record["message"] === "string" &&
    typeof record["file"] === "string" &&
    typeof record["line"] === "number"
  ) {
    return {
      event: "case",
      path: record["path"],
      status: "fail",
      message: record["message"],
      file: record["file"],
      line: record["line"],
    };
  }
  return undefined;
}
