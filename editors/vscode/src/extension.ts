// Quilon VS Code extension.
//
// Four responsibilities:
//   1. Commands that run the Quilon compiler on the active .qn file in a terminal
//      ("Quilon: Check / Run Current File", "Quilon: Run Tests in Current File").
//   2. The language client: spawns `quilon lsp` (the compiler's own language server)
//      and lets it provide diagnostics, go-to-definition, hover, semantic tokens,
//      and the test code lenses.
//   3. Debugging (`debug.ts`): the `quilon.debug` command and debug-configuration provider
//      (CodeLLDB over a `--debug` build), plus `quilon.debugTests` for the language
//      server's Debug suite/case lenses (CodeLLDB over a `quilon test --binary` build).
//   4. The Test Explorer (`testExplorer.ts`): the "Testing" view's tree, built from
//      the language server's `quilon/testItems`, with a Run profile that parses
//      `quilon test --reporter json` back into pass/fail results, and a Debug profile
//      that builds and launches the selection under CodeLLDB.

import * as fs from "node:fs";
import * as os from "node:os";
import * as vscode from "vscode";
import { LanguageClient, type ServerOptions } from "vscode-languageclient/node";
import {
  type CompilerProbe,
  languageServerInvocation,
  missingCompilerMessage,
  resolveCompilerCommand,
  type ResolvedCompiler,
  shellCommand,
} from "./compilerCommand";
import { registerDebug } from "./debug";
import { findEntryPoints } from "./entryPoints";
import { registerTestExplorer } from "./testExplorer";

// --- Locating the compiler -------------------------------------------------

/** The `quilon.command` value the user set, or "" when it is at its default. */
function configuredCommand(): string {
  const inspected = vscode.workspace.getConfiguration("quilon").inspect<string>("command");
  const set =
    inspected?.workspaceFolderValue ?? inspected?.workspaceValue ?? inspected?.globalValue ?? "";
  return typeof set === "string" ? set : "";
}

/** The machine as the resolver sees it: the setting, this host's environment, and fs probes. */
function compilerProbe(): CompilerProbe {
  return {
    configured: configuredCommand(),
    path: process.env["PATH"],
    pathExt: process.env["PATHEXT"],
    home: os.homedir(),
    workspaceFolders: (vscode.workspace.workspaceFolders ?? []).map((f) => f.uri.fsPath),
    platform: process.platform,
    isExecutable: (file) => {
      try {
        if (!fs.statSync(file).isFile()) {
          return false;
        }
        fs.accessSync(file, fs.constants.X_OK);
        return true;
      } catch {
        return false;
      }
    },
    readText: (file) => {
      try {
        return fs.readFileSync(file, "utf8");
      } catch {
        return undefined;
      }
    },
  };
}

/**
 * The resolved invocation, cached for the session: resolution touches the disk,
 * and every check/run/debug needs it. Invalidated when the setting or the open
 * folders change, and whenever a spawn fails — so installing a compiler
 * mid-session is picked up on the next attempt.
 */
let cachedCompiler: ResolvedCompiler | undefined;

/** How to invoke the compiler — the setting when set, otherwise a located one. */
export function resolvedQuilonCompiler(): ResolvedCompiler {
  cachedCompiler ??= resolveCompilerCommand(compilerProbe());
  return cachedCompiler;
}

/** Drop the cached invocation so the next call resolves afresh. */
export function forgetResolvedCompiler(): void {
  cachedCompiler = undefined;
}

/**
 * Report a compiler that could not be spawned, offering the setting that fixes
 * it. `severity` keeps a background failure a warning while a user-initiated
 * action reports an error.
 */
export function showMissingCompiler(
  resolved: ResolvedCompiler,
  prefix: string,
  severity: "warning" | "error" = "error",
): void {
  const message = `Quilon: ${prefix}${missingCompilerMessage(resolved)}`;
  const shown =
    severity === "warning"
      ? vscode.window.showWarningMessage(message, "Open Settings")
      : vscode.window.showErrorMessage(message, "Open Settings");
  void shown.then((choice) => {
    if (choice === "Open Settings") {
      void vscode.commands.executeCommand("workbench.action.openSettings", "quilon.command");
    }
  });
}

// --- Terminal commands -----------------------------------------------------

/** The shared "Quilon" terminal, created on first use. */
function quilonTerminal(): vscode.Terminal {
  return (
    vscode.window.terminals.find((t) => t.name === "Quilon") ??
    vscode.window.createTerminal("Quilon")
  );
}

function runOnActiveFile(subcommand: string): void {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "quilon") {
    void vscode.window.showErrorMessage("Quilon: no active .qn file.");
    return;
  }
  const document = editor.document;
  // Save first so the compiler sees the latest content.
  void document.save().then(() => {
    const cmd = shellCommand(resolvedQuilonCompiler());
    const file = document.fileName;
    const term = quilonTerminal();
    term.show();
    // Quote the path to tolerate spaces.
    term.sendText(`${cmd} ${subcommand} "${file}"`);
  });
}

/**
 * Run `quilon test` on a file, in the shared terminal. The test code lenses the language
 * server places above `describe`/`it` blocks invoke this with the file's path and the
 * block's own `/`-joined path, so a suite lens runs just that suite and a case lens just
 * that case (`--only <testPath>`); invoked bare (from the command palette, with no
 * `testPath`) it targets the active file's whole suite set.
 */
async function runTestsInFile(filePath?: string, testPath?: string): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  const target = filePath ?? editor?.document.fileName;
  if (target === undefined) {
    void vscode.window.showErrorMessage("Quilon: no .qn file to test.");
    return;
  }
  // The compiler reads the file from disk; save the buffer it is about to test.
  if (editor && editor.document.fileName === target && editor.document.isDirty) {
    await editor.document.save();
  }
  const cmd = shellCommand(resolvedQuilonCompiler());
  const term = quilonTerminal();
  term.show();
  const only = testPath ? ` --only "${testPath}"` : "";
  term.sendText(`${cmd} test "${target}"${only}`);
}

// --- The language client ---------------------------------------------------

let client: LanguageClient | undefined;

/**
 * Spawn `quilon lsp` with the resolved compiler invocation and connect the
 * editor to it. Diagnostics, go-to-definition, hover, completion (triggered on
 * `.`), semantic tokens, and the test code lenses all come from the server; a
 * failure to start is reported once, with the setting that fixes it.
 */
async function startLanguageClient(): Promise<void> {
  const compiler = resolvedQuilonCompiler();
  const serverOptions: ServerOptions = languageServerInvocation(compiler);
  client = new LanguageClient("quilon", "Quilon Language Server", serverOptions, {
    documentSelector: [{ language: "quilon" }],
  });
  try {
    await client.start();
  } catch {
    client = undefined;
    forgetResolvedCompiler();
    showMissingCompiler(compiler, "language server unavailable — ", "warning");
  }
}

/** Stop the running client (if any) and start a fresh one — after a setting change. */
async function restartLanguageClient(): Promise<void> {
  const running = client;
  client = undefined;
  if (running) {
    await running.stop().then(undefined, () => {});
  }
  await startLanguageClient();
}

// --- CodeLens: Run above each `^` entry point ------------------------------

/**
 * Places "▶ Run" and "▶ Debug" actions above every top-level `^` entry-point
 * definition. They invoke the `quilon.run` / `quilon.debug` commands, which act
 * on the active editor — and since the lenses live in that document, clicking
 * one (which focuses the doc) targets the right file without threading the URI
 * through. Debug builds the file with `--debug` and launches it under CodeLLDB
 * so breakpoints in the `.qn` source are hit. (The test lenses above
 * `describe`/`it` blocks come from the language server instead.)
 */
class EntryPointCodeLensProvider implements vscode.CodeLensProvider {
  provideCodeLenses(document: vscode.TextDocument): vscode.CodeLens[] {
    const lenses: vscode.CodeLens[] = [];
    for (const entry of findEntryPoints(document.getText())) {
      const range = new vscode.Range(entry.line, entry.column, entry.line, entry.column + 1);
      lenses.push(
        new vscode.CodeLens(range, {
          title: "▶ Run",
          command: "quilon.run",
          tooltip: "Run this Quilon program",
        }),
        new vscode.CodeLens(range, {
          title: "▶ Debug",
          command: "quilon.debug",
          tooltip: "Debug this Quilon program (breakpoints, stepping)",
        }),
      );
    }
    return lenses;
  }
}

export function activate(context: vscode.ExtensionContext): void {
  // Debug integration (CodeLLDB): the `quilon.debug` command and the `quilon`
  // debug-configuration provider that builds with `--debug` and launches lldb.
  registerDebug(context);

  context.subscriptions.push(
    vscode.commands.registerCommand("quilon.check", () => runOnActiveFile("check")),
    vscode.commands.registerCommand("quilon.run", () => runOnActiveFile("run")),
    vscode.commands.registerCommand("quilon.runTests", (filePath?: string, testPath?: string) =>
      runTestsInFile(filePath, testPath),
    ),
    vscode.languages.registerCodeLensProvider(
      { language: "quilon" },
      new EntryPointCodeLensProvider(),
    ),
    // A changed setting or a changed set of folders can change which compiler
    // we should be running, so drop the cached resolution and reconnect the
    // language server to the right one.
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("quilon.command")) {
        forgetResolvedCompiler();
        void restartLanguageClient();
      }
    }),
    vscode.workspace.onDidChangeWorkspaceFolders(() => forgetResolvedCompiler()),
  );

  // The Test Explorer: its tree comes from the language server's `quilon/testItems`
  // request, so it needs the running client — read lazily since the client starts
  // asynchronously below and may restart later.
  registerTestExplorer(context, () => client, resolvedQuilonCompiler);

  void startLanguageClient();
}

export function deactivate(): Thenable<void> | undefined {
  forgetResolvedCompiler();
  const running = client;
  client = undefined;
  return running?.stop();
}
