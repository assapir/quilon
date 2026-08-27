// Quilon VS Code extension.
//
// Two responsibilities:
//   1. Commands that run the Quilon compiler on the active .qn file in a terminal
//      ("Quilon: Check / Run Current File").
//   2. Inline diagnostics: on open/save of a .qn file, run `<command> check` on
//      it, parse the rustc-style `path:line:col: error: message` output, and
//      surface each as an editor squiggle in a shared DiagnosticCollection.

import { execFile } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as vscode from "vscode";
import {
  type CompilerProbe,
  missingCompilerMessage,
  resolveCompilerCommand,
  type ResolvedCompiler,
  shellCommand,
} from "./compilerCommand";
import { registerDebug } from "./debug";
import { firstNonEmptyLine } from "./debugConfig";
import { parseDiagnostics, type ParsedDiagnostic } from "./diagnostics";
import { findEntryPoints } from "./entryPoints";

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
 * it. `severity` keeps a background diagnostics failure a warning while a
 * user-initiated action reports an error.
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
    const term =
      vscode.window.terminals.find((t) => t.name === "Quilon") ??
      vscode.window.createTerminal("Quilon");
    term.show();
    // Quote the path to tolerate spaces.
    term.sendText(`${cmd} ${subcommand} "${file}"`);
  });
}

// --- Inline diagnostics ----------------------------------------------------

/**
 * True once we've warned that the compiler is missing, so we don't spam. Reset
 * after any successful run so a later re-break (command removed again) warns
 * afresh rather than failing silently.
 */
let warnedMissingCommand = false;

/**
 * Monotonic check counter per file URI. A check stamps its sequence number and,
 * on completion, only publishes if it is still the latest — so an older run that
 * finishes after a newer one can't overwrite fresh results with stale ones.
 */
const latestCheck = new Map<string, number>();

/** Convert one parsed diagnostic into a VS Code diagnostic against `document`. */
function toVsDiagnostic(
  parsed: ParsedDiagnostic,
  document: vscode.TextDocument,
): vscode.Diagnostic {
  // Compiler line/column are 1-based; VS Code positions are 0-based.
  const line = Math.max(0, parsed.line - 1);
  const startChar = Math.max(0, parsed.column - 1);
  const start = new vscode.Position(line, startChar);

  // Prefer the caret-run width; otherwise underline the token at the column,
  // and if even that is empty, fall back to a one-character range so the
  // squiggle is visible.
  let end: vscode.Position;
  if (parsed.span !== undefined && parsed.span > 0) {
    end = new vscode.Position(line, startChar + parsed.span);
  } else {
    const wordRange = document.getWordRangeAtPosition(start);
    end =
      wordRange && wordRange.end.isAfter(start)
        ? wordRange.end
        : new vscode.Position(line, startChar + 1);
  }

  const diagnostic = new vscode.Diagnostic(
    new vscode.Range(start, end),
    parsed.message,
    vscode.DiagnosticSeverity.Error,
  );
  diagnostic.source = "quilon";
  return diagnostic;
}

/**
 * Run `<command> check <file>` and publish the resulting diagnostics for
 * `document`. Clears them when the file checks clean. Robust to a missing
 * command (warns once), non-zero exit, out-of-order completion of overlapping
 * runs, and multi-error output.
 */
function checkDocument(
  document: vscode.TextDocument,
  collection: vscode.DiagnosticCollection,
): void {
  if (document.languageId !== "quilon" || document.uri.scheme !== "file") {
    return;
  }

  // The compiler reads the file from disk, so a dirty buffer would yield
  // diagnostics positioned against stale text. Skip until it's saved (the
  // on-save handler re-runs once the buffer and disk agree).
  if (document.isDirty) {
    return;
  }

  const compiler = resolvedQuilonCompiler();
  const cwd = vscode.workspace.getWorkspaceFolder(document.uri)?.uri.fsPath;

  // Stamp this run so a slower earlier run can't clobber a faster later one.
  const uri = document.uri;
  const key = uri.toString();
  const seq = (latestCheck.get(key) ?? 0) + 1;
  latestCheck.set(key, seq);

  const args = [...compiler.baseArgs, "check", document.fileName];
  execFile(compiler.exe, args, { cwd }, (error, stdout, stderr) => {
    if (latestCheck.get(key) !== seq) {
      return; // A newer check superseded this one; drop its (stale) result.
    }

    // ENOENT => the compiler isn't where we resolved it. Forget the resolution
    // so a compiler installed mid-session is found on the next check.
    if (error && (error as NodeJS.ErrnoException).code === "ENOENT") {
      forgetResolvedCompiler();
      if (!warnedMissingCommand) {
        warnedMissingCommand = true;
        showMissingCompiler(compiler, "diagnostics unavailable — ", "warning");
      }
      collection.delete(uri);
      return;
    }

    // We ran the compiler successfully; allow a fresh warning if it later breaks.
    warnedMissingCommand = false;

    // Parse the rustc-style report. Diagnostics go to stderr; include stdout
    // for forward-compatibility.
    const combined = `${stderr}\n${stdout}`;
    const parsed = parseDiagnostics(combined);
    if (parsed.length > 0) {
      collection.set(
        uri,
        parsed.map((d) => toVsDiagnostic(d, document)),
      );
      return;
    }

    // No parseable diagnostics. A zero exit means a clean check; a non-zero exit
    // with unrecognized output (a panic/backtrace, or a future message format)
    // is still a failure — surface it rather than silently reporting "clean".
    if (error) {
      collection.set(uri, [unparsedFailureDiagnostic(combined)]);
    } else {
      collection.delete(uri);
    }
  });
}

/**
 * A whole-file diagnostic for a compiler failure whose output we couldn't parse
 * into a located error, so the user still sees that the check failed.
 */
function unparsedFailureDiagnostic(output: string): vscode.Diagnostic {
  const detail = firstNonEmptyLine(output) ?? "unknown error";
  const diagnostic = new vscode.Diagnostic(
    new vscode.Range(0, 0, 0, 0),
    `Quilon check failed: ${detail}`,
    vscode.DiagnosticSeverity.Error,
  );
  diagnostic.source = "quilon";
  return diagnostic;
}

// --- CodeLens: Run above each `^` entry point ------------------------------

/**
 * Places "▶ Run" and "▶ Debug" actions above every top-level `^` entry-point
 * definition. They invoke the `quilon.run` / `quilon.debug` commands, which act
 * on the active editor — and since the lenses live in that document, clicking
 * one (which focuses the doc) targets the right file without threading the URI
 * through. Debug builds the file with `--debug` and launches it under CodeLLDB
 * so breakpoints in the `.qn` source are hit.
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
  const diagnostics = vscode.languages.createDiagnosticCollection("quilon");
  context.subscriptions.push(diagnostics);

  const check = (document: vscode.TextDocument): void => checkDocument(document, diagnostics);

  // Debug integration (CodeLLDB): the `quilon.debug` command and the `quilon`
  // debug-configuration provider that builds with `--debug` and launches lldb.
  registerDebug(context);

  context.subscriptions.push(
    vscode.commands.registerCommand("quilon.check", () => runOnActiveFile("check")),
    vscode.commands.registerCommand("quilon.run", () => runOnActiveFile("run")),
    vscode.languages.registerCodeLensProvider(
      { language: "quilon" },
      new EntryPointCodeLensProvider(),
    ),
    // A changed setting or a changed set of folders can change which compiler
    // we should be running, so drop the cached resolution and re-check.
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("quilon.command")) {
        forgetResolvedCompiler();
        const document = vscode.window.activeTextEditor?.document;
        if (document) {
          check(document);
        }
      }
    }),
    vscode.workspace.onDidChangeWorkspaceFolders(() => forgetResolvedCompiler()),
    vscode.workspace.onDidOpenTextDocument(check),
    vscode.workspace.onDidSaveTextDocument(check),
    // Re-check when focus lands on a file (e.g. switching to an already-open tab
    // that was never saved this session), so its diagnostics are current.
    vscode.window.onDidChangeActiveTextEditor((editor) => {
      if (editor) {
        check(editor.document);
      }
    }),
    vscode.workspace.onDidCloseTextDocument((document) => {
      diagnostics.delete(document.uri);
      latestCheck.delete(document.uri.toString());
    }),
  );

  // Check the active editor's document on activation. Other already-open files
  // get checked lazily on their first save (or when re-opened), avoiding a
  // startup stampede of compiler processes — which, with the documented
  // `cargo run --` setting, would otherwise all contend on the same build lock.
  const active = vscode.window.activeTextEditor?.document;
  if (active) {
    check(active);
  }
}

export function deactivate(): void {
  // The DiagnosticCollection is disposed via context.subscriptions; reset the
  // module-level state so a re-activation in the same host starts clean.
  latestCheck.clear();
  warnedMissingCommand = false;
  forgetResolvedCompiler();
}
