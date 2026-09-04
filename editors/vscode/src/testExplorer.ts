// The Quilon Test Explorer: a `vscode.tests.TestController` built from the language
// server's `quilon/testItems` request (the same data the ▶ Run/▶ Run suite/▶ Run case
// CodeLens read), with a Run profile that spawns `quilon test <file> --reporter json
// [--only <path>]` and parses the NDJSON event stream back into pass/fail results.
//
// Its Debug profile builds each selected item into a native, debuggable executable
// (`quilon test <file> [--only <path>] --binary <tmpbin>`, `--binary` carrying debug info
// implicitly — see `src/test_command.rs`) and launches it under CodeLLDB, one session per
// selected item, sequentially: `debug.ts`'s `buildDebuggable` is the same build-and-launch
// step the 🐞 Debug suite / 🐞 Debug case CodeLens use. Unlike Run, a debug session reports
// no pass/fail here — the point is to step through it — so a debugged item is only marked
// started, not passed or failed.

import { type ChildProcessWithoutNullStreams, spawn } from "node:child_process";
import * as path from "node:path";
import * as vscode from "vscode";
import { type LanguageClient } from "vscode-languageclient/node";
import { type ResolvedCompiler } from "./compilerCommand";
import { buildDebuggable, saveIfDirty } from "./debug";
import { tempBinaryPath, testBuildArgs } from "./debugConfig";
import {
  buildTestTree,
  parseReporterLine,
  testRunArgs,
  type TestItemInfo,
  type TestNode,
} from "./testRunner";

/** The suite/case `/`-joined path behind a `vscode.TestItem` — absent for a file-root item. */
const testPaths = new WeakMap<vscode.TestItem, string>();

/**
 * Registers the "Quilon" test controller: it fills from `quilon/testItems` on open/change/
 * save of a `.qn` document, and its Run profile executes the selection. `getClient` reads
 * the language client lazily — it may not have started yet, or may be mid-restart.
 */
export function registerTestExplorer(
  context: vscode.ExtensionContext,
  getClient: () => LanguageClient | undefined,
  resolveCompiler: () => ResolvedCompiler,
): vscode.TestController {
  const controller = vscode.tests.createTestController("quilon", "Quilon");
  context.subscriptions.push(controller);

  const refresh = (document: vscode.TextDocument) =>
    void refreshDocument(controller, getClient, document);
  const debounced = debounce(refresh, 300);

  controller.refreshHandler = async () => {
    await Promise.all(
      vscode.workspace.textDocuments.map((document) =>
        refreshDocument(controller, getClient, document),
      ),
    );
  };

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(refresh),
    vscode.workspace.onDidSaveTextDocument(refresh),
    vscode.workspace.onDidChangeTextDocument((event) => debounced(event.document)),
    vscode.workspace.onDidCloseTextDocument((document) => {
      controller.items.delete(document.uri.toString());
    }),
  );
  for (const document of vscode.workspace.textDocuments) {
    refresh(document);
  }

  context.subscriptions.push(
    controller.createRunProfile(
      "Run",
      vscode.TestRunProfileKind.Run,
      (request, token) => void runHandler(controller, resolveCompiler, request, token),
      true,
    ),
    controller.createRunProfile(
      "Debug",
      vscode.TestRunProfileKind.Debug,
      (request, token) => void debugHandler(controller, resolveCompiler, context, request, token),
      true,
    ),
  );

  return controller;
}

/** Ask the server for `document`'s test tree and replace what the controller shows for it. */
async function refreshDocument(
  controller: vscode.TestController,
  getClient: () => LanguageClient | undefined,
  document: vscode.TextDocument,
): Promise<void> {
  if (document.languageId !== "quilon" || document.uri.scheme !== "file") {
    return;
  }
  const client = getClient();
  if (!client) {
    return;
  }
  let items: TestItemInfo[];
  try {
    items = await client.sendRequest<TestItemInfo[]>("quilon/testItems", {
      textDocument: { uri: client.code2ProtocolConverter.asUri(document.uri) },
    });
  } catch {
    // The server may not be up yet, or the document may not parse — leave the tree as it
    // was rather than clearing it out from under a run.
    return;
  }
  syncFileTests(controller, document.uri, items);
}

/** Replace the test tree the controller shows for `uri` with `items`, or drop it when empty. */
function syncFileTests(
  controller: vscode.TestController,
  uri: vscode.Uri,
  items: readonly TestItemInfo[],
): void {
  const fileId = uri.toString();
  if (items.length === 0) {
    controller.items.delete(fileId);
    return;
  }
  const fileItem = controller.createTestItem(fileId, path.basename(uri.fsPath), uri);
  fileItem.children.replace(buildTestTree(items).map((node) => toTestItem(controller, uri, node)));
  controller.items.add(fileItem);
}

// A test item's id is the file URI and the suite/case's own `/`-joined path, separated by
// `\n`: a path is a sequence of text-literal names (never containing a raw newline) and a
// URI can't contain one either, so `\n` can't collide with either half — unlike `::` or a
// space, which a suite/case NAME could legitimately contain.
const ID_SEPARATOR = "\n";

function toTestItem(
  controller: vscode.TestController,
  uri: vscode.Uri,
  node: TestNode,
): vscode.TestItem {
  const item = controller.createTestItem(
    `${uri.toString()}${ID_SEPARATOR}${node.path}`,
    node.name,
    uri,
  );
  item.range = new vscode.Range(
    node.range.start.line,
    node.range.start.character,
    node.range.end.line,
    node.range.end.character,
  );
  testPaths.set(item, node.path);
  item.children.replace(node.children.map((child) => toTestItem(controller, uri, child)));
  return item;
}

/** One file's worth of a run request: its selected items, grouped by their document. */
interface FileGroup {
  uri: vscode.Uri;
  items: vscode.TestItem[];
}

async function runHandler(
  controller: vscode.TestController,
  resolveCompiler: () => ResolvedCompiler,
  request: vscode.TestRunRequest,
  token: vscode.CancellationToken,
): Promise<void> {
  const run = controller.createTestRun(request);
  const selected = request.include ?? collectTopLevel(controller);
  const excluded = new Set(request.exclude ?? []);
  const groups = groupByFile(selected.filter((item) => !excluded.has(item)));

  for (const [item] of walkAll(selected)) {
    run.enqueued(item);
  }

  // One `quilon test` process per file, in parallel — each carries its own JIT process
  // and its own selection, so the runs don't interact; `runFile` itself kills its child
  // on cancellation.
  await Promise.all(
    [...groups.values()].map((group) => runFile(run, group, resolveCompiler, token)),
  );
  run.end();
}

/**
 * The Debug profile: one `quilon test --binary` build and CodeLLDB session per selected
 * item, in turn — a suite or case item builds with `--only <path>`, a file-root item (no
 * path of its own) builds the whole file. Sequential, unlike Run's parallel-per-file
 * spawns, so debug sessions never pile up on top of each other; each item is marked started
 * before its session launches but carries no pass/fail verdict, since stepping through it
 * by hand is the point.
 */
async function debugHandler(
  controller: vscode.TestController,
  resolveCompiler: () => ResolvedCompiler,
  context: vscode.ExtensionContext,
  request: vscode.TestRunRequest,
  token: vscode.CancellationToken,
): Promise<void> {
  const run = controller.createTestRun(request);
  const excluded = new Set(request.exclude ?? []);
  const selected = (request.include ?? collectTopLevel(controller)).filter(
    (item) => !excluded.has(item),
  );

  // Chained rather than a `for`-`await` loop, so each session's build-and-launch only
  // starts once the previous one has ended (a plain loop with an internal `await` reads the
  // same but is indistinguishable from an accidentally-serialized parallel task).
  await selected.reduce(async (previous, item) => {
    await previous;
    if (token.isCancellationRequested || !item.uri) {
      return;
    }
    run.started(item);
    await debugOneItem(context, resolveCompiler, item.uri.fsPath, testPaths.get(item));
  }, Promise.resolve());
  run.end();
}

/**
 * Build one suite/case (or the whole file, when `testPath` is `undefined` — a file-root
 * item) for debug and run it under CodeLLDB, awaiting the session's end before returning so
 * the caller's sequential loop doesn't start the next one on top of it.
 */
async function debugOneItem(
  context: vscode.ExtensionContext,
  resolveCompiler: () => ResolvedCompiler,
  file: string,
  testPath: string | undefined,
): Promise<void> {
  const document = vscode.workspace.textDocuments.find((d) => d.uri.fsPath === file);
  if (document) {
    await saveIfDirty(file);
  }

  const compiler = resolveCompiler();
  const output = tempBinaryPath(file);
  const folder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(file));
  const cwd = folder?.uri.fsPath;
  const sessionName = testPath ?? path.basename(file);

  const config = await buildDebuggable(
    context,
    compiler,
    testBuildArgs(compiler.baseArgs, file, testPath, output),
    output,
    cwd,
    `Quilon: building ${path.basename(file)} for debug…`,
    `Quilon: Debug ${sessionName}`,
  );
  if (!config) {
    return;
  }

  // Subscribed BEFORE starting the session: a session with no breakpoint hit can run to
  // completion and terminate before an `await`ed `startDebugging` even resolves, and a
  // subscription installed only after that would miss the termination event and hang here
  // forever.
  let subscription: vscode.Disposable | undefined;
  const ended = new Promise<void>((resolve) => {
    subscription = vscode.debug.onDidTerminateDebugSession((session) => {
      if (session.configuration.program === output) {
        subscription?.dispose();
        resolve();
      }
    });
  });

  const started = await vscode.debug.startDebugging(folder, config);
  if (!started) {
    subscription?.dispose();
    return;
  }
  await ended;
}

function collectTopLevel(controller: vscode.TestController): vscode.TestItem[] {
  const items: vscode.TestItem[] = [];
  controller.items.forEach((item) => items.push(item));
  return items;
}

function groupByFile(selected: readonly vscode.TestItem[]): Map<string, FileGroup> {
  const groups = new Map<string, FileGroup>();
  for (const item of selected) {
    if (!item.uri) {
      continue;
    }
    const key = item.uri.toString();
    const group = groups.get(key) ?? { uri: item.uri, items: [] };
    group.items.push(item);
    groups.set(key, group);
  }
  return groups;
}

/** `item` and every descendant, depth-first — what a run needs to enqueue and to index. */
function* walkAll(items: readonly vscode.TestItem[]): Generator<[vscode.TestItem]> {
  for (const item of items) {
    yield [item];
    const children: vscode.TestItem[] = [];
    item.children.forEach((child) => children.push(child));
    yield* walkAll(children);
  }
}

/**
 * Run one file's selection: `quilon test <file> --reporter json`, `--only <path>` per
 * selected item unless the file-root item itself is among them (a whole-file run). Streams
 * stdout, applying each parsed event to `run` as it arrives.
 */
async function runFile(
  run: vscode.TestRun,
  group: FileGroup,
  resolveCompiler: () => ResolvedCompiler,
  token: vscode.CancellationToken,
): Promise<void> {
  const file = group.uri.fsPath;
  const onlyPaths = group.items
    .map((item) => testPaths.get(item))
    .filter((selectedPath): selectedPath is string => selectedPath !== undefined);
  const wholeFile = onlyPaths.length !== group.items.length;
  const index = indexByPath(group.items);

  const document = vscode.workspace.textDocuments.find(
    (d) => d.uri.toString() === group.uri.toString(),
  );
  if (document) {
    await saveIfDirty(file);
  }

  const compiler = resolveCompiler();
  const args = testRunArgs(compiler.baseArgs, file, wholeFile ? [] : onlyPaths);
  const cwd = vscode.workspace.getWorkspaceFolder(group.uri)?.uri.fsPath;

  let child: ChildProcessWithoutNullStreams;
  try {
    child = spawn(compiler.exe, args, { cwd });
  } catch {
    reportSpawnFailure(run, group.items);
    return;
  }

  const onCancel = token.onCancellationRequested(() => child.kill());
  let buffer = "";
  let spawnFailed = false;
  child.on("error", () => {
    spawnFailed = true;
  });
  child.stdout.on("data", (chunk: Buffer) => {
    buffer += chunk.toString("utf8");
    const lines = buffer.split("\n");
    buffer = lines.pop() ?? "";
    for (const line of lines) {
      applyEvent(run, index, parseReporterLine(line));
    }
  });

  await new Promise<void>((resolve) => child.once("close", () => resolve()));
  onCancel.dispose();
  applyEvent(run, index, parseReporterLine(buffer));
  if (spawnFailed) {
    reportSpawnFailure(run, group.items);
  }
}

/** Path → item for every selected item and its descendants — what run events look up. */
function indexByPath(items: readonly vscode.TestItem[]): Map<string, vscode.TestItem> {
  const index = new Map<string, vscode.TestItem>();
  for (const [item] of walkAll(items)) {
    const itemPath = testPaths.get(item);
    if (itemPath !== undefined) {
      index.set(itemPath, item);
    }
  }
  return index;
}

function applyEvent(
  run: vscode.TestRun,
  index: Map<string, vscode.TestItem>,
  event: ReturnType<typeof parseReporterLine>,
): void {
  if (!event || event.event === "summary") {
    return;
  }
  const item = index.get(event.path);
  if (!item) {
    return;
  }
  if (event.event === "suite") {
    run.started(item);
    return;
  }
  if (event.status === "pass") {
    run.passed(item);
    return;
  }
  const message = new vscode.TestMessage(event.message);
  message.location = new vscode.Location(
    vscode.Uri.file(event.file),
    new vscode.Position(Math.max(0, event.line - 1), 0),
  );
  run.failed(item, message);
}

function reportSpawnFailure(run: vscode.TestRun, items: readonly vscode.TestItem[]): void {
  const message = new vscode.TestMessage(
    'Quilon: could not run the compiler — check the "quilon.command" setting.',
  );
  for (const [item] of walkAll(items)) {
    run.errored(item, message);
  }
}

/** Coalesce calls to `fn` arriving within `delayMs` of each other, per document. */
function debounce(
  fn: (document: vscode.TextDocument) => void,
  delayMs: number,
): (document: vscode.TextDocument) => void {
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  return (document) => {
    const key = document.uri.toString();
    const existing = timers.get(key);
    if (existing) {
      clearTimeout(existing);
    }
    timers.set(
      key,
      setTimeout(() => {
        timers.delete(key);
        fn(document);
      }, delayMs),
    );
  };
}
