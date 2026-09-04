// Quilon debug integration.
//
// Debugging is delegated to CodeLLDB (`vadimcn.vscode-lldb`, a declared
// extension dependency). We contribute a `quilon` debug type whose
// DebugConfigurationProvider, when a session starts, builds the active `.qn`
// with `<command> build --debug <file> -o <tmpbin>` and then resolves into a
// CodeLLDB (`type: "lldb"`) launch of that binary. Breakpoints set in the `.qn`
// source are hit through the DWARF line table the `--debug` build emits.
//
// The `quilon.debugTests` command (the language server's 🐞 Debug suite / 🐞 Debug case
// lenses) and the Test Explorer's Debug profile (`testExplorer.ts`) build a suite the same
// way but with `quilon test <file> --only <path> --binary <tmpbin>` instead — a `--binary`
// build carries debug info implicitly. Both paths share `buildDebuggable` below: build,
// surface a failure the same way, resolve the CodeLLDB launch of the result.
//
// Value rendering (Text as a string, `[]T` as an indexed list) is provided by
// the lldb formatter we import against the compiler's DWARF types — see
// `formatters/quilon.py`.

import { execFile } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import { type LanguageClient } from "vscode-languageclient/node";
import { type ResolvedCompiler } from "./compilerCommand";
import {
  buildArgs,
  firstNonEmptyLine,
  InFlightBuilds,
  tempBinaryPath,
  testBuildArgs,
  toLldbConfiguration,
} from "./debugConfig";
import { forgetResolvedCompiler, resolvedQuilonCompiler, showMissingCompiler } from "./extension";

/** The CodeLLDB extension we delegate to; also declared in extensionDependencies. */
const CODELLDB_ID = "vadimcn.vscode-lldb";

/**
 * Path to the shipped lldb formatter, or undefined if it isn't present (e.g. a
 * stripped package). A missing formatter must not break a debug session — it
 * only costs the pretty value rendering.
 */
function formatterPath(context: vscode.ExtensionContext): string | undefined {
  const p = path.join(context.extensionPath, "formatters", "quilon.py");
  return fs.existsSync(p) ? p : undefined;
}

/**
 * Ask the running language client for the directory it materialized the embedded corelib
 * modules into (`quilon/corelibDir`), for the CodeLLDB `sourceMap` that lets stepping into
 * corelib code show real source (see `corelibSourceMap` in `debugConfig.ts`). `undefined` on
 * any failure — no client running, the server doesn't answer the request, or the write
 * failed on its end — which just costs corelib source, not the debug session: the caller
 * starts it regardless.
 */
async function requestCorelibDir(client: LanguageClient | undefined): Promise<string | undefined> {
  if (!client) {
    return undefined;
  }
  try {
    return await client.sendRequest<string>("quilon/corelibDir");
  } catch {
    return undefined;
  }
}

/**
 * Resolve the `.qn` source to debug from the launch config and the active
 * editor. A configured `program` wins (variables are already substituted by the
 * time this runs); otherwise fall back to the active `.qn` editor so a bare F5
 * or the Debug CodeLens works without a launch.json.
 */
function resolveSourceFile(config: vscode.DebugConfiguration): string | undefined {
  const fromConfig = typeof config.program === "string" ? config.program.trim() : "";
  if (fromConfig.length > 0) {
    return fromConfig;
  }
  const active = vscode.window.activeTextEditor?.document;
  if (active && active.languageId === "quilon" && active.uri.scheme === "file") {
    return active.uri.fsPath;
  }
  return undefined;
}

/**
 * Flush a dirty buffer for `file` so the build sees the latest text. Reports its
 * own error (a save failure is not a build failure) and returns whether the file
 * is on disk and ready to build.
 */
export async function saveIfDirty(file: string): Promise<boolean> {
  const open = vscode.workspace.textDocuments.find((d) => d.uri.fsPath === file);
  if (!open?.isDirty) {
    return true;
  }
  try {
    await open.save();
    return true;
  } catch (error) {
    void vscode.window.showErrorMessage(
      `Quilon: could not save ${path.basename(file)}: ${(error as Error).message}`,
    );
    return false;
  }
}

/** Thrown when the compiler itself could not be spawned, as opposed to failing to build. */
class CompilerMissing extends Error {
  constructor(readonly compiler: ResolvedCompiler) {
    super("compiler not found");
  }
}

/** Run the compiler with `args`; reject with `CompilerMissing` when it can't be spawned at
 * all, or with its first stderr line on any other failure. */
function runCompiler(
  compiler: ResolvedCompiler,
  args: string[],
  cwd: string | undefined,
): Promise<void> {
  return new Promise((resolve, reject) => {
    execFile(compiler.exe, args, { cwd }, (error, _stdout, stderr) => {
      if (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") {
          reject(new CompilerMissing(compiler));
          return;
        }
        reject(new Error(firstNonEmptyLine(stderr) ?? error.message));
        return;
      }
      resolve();
    });
  });
}

/**
 * The optional trailing inputs to {@link buildDebuggable}, grouped so a caller that only
 * needs some of them (or none) doesn't have to fill in the others positionally.
 */
export interface DebuggableOptions {
  /** Arguments forwarded to the program (its `^` `args`). Defaults to none. */
  programArgs?: string[];
  /**
   * The `.qn` source being debugged. Together with `getClient`, lets this ask
   * `quilon/corelibDir` and add the resulting `sourceMap`, so stepping into a corelib
   * function shows real source; omitted (or the request failing), the session still starts
   * without it.
   */
  sourceFile?: string;
  /** Reads the running language client, when up — see `sourceFile`. */
  getClient?: () => LanguageClient | undefined;
}

/**
 * Build a debuggable native executable by running `<compiler> buildArgv`, surfacing any
 * failure the way every Quilon debug entry point does — a missing-CodeLLDB notice before
 * anything is built, a progress notification while the build runs, a missing-compiler
 * notice, or the compiler's own first error line — then resolve the CodeLLDB
 * (`type: "lldb"`) launch of `output`. `undefined` on any failure, already reported to the
 * user.
 *
 * Shared by the `quilon` debug-configuration provider below (`quilon build --debug`) and the
 * test Debug lenses/profile (`quilon test --binary`, in `extension.ts` and
 * `testExplorer.ts`), which differ only in the argv that produces the binary and in what
 * they do with the resolved configuration — the provider returns it for VS Code to launch,
 * a direct command starts the session itself. The CodeLLDB check lives here rather than in
 * each caller so every path fails fast, before spending a build, on the one setup problem
 * they all share.
 */
export async function buildDebuggable(
  context: vscode.ExtensionContext,
  compiler: ResolvedCompiler,
  buildArgv: string[],
  output: string,
  cwd: string | undefined,
  progressTitle: string,
  sessionName: string,
  options: DebuggableOptions = {},
): Promise<vscode.DebugConfiguration | undefined> {
  const { programArgs = [], sourceFile, getClient } = options;
  if (!vscode.extensions.getExtension(CODELLDB_ID)) {
    void vscode.window.showErrorMessage(
      "Quilon debugging needs the CodeLLDB extension (vadimcn.vscode-lldb). Install it and try again.",
    );
    return undefined;
  }
  // Started alongside the build, not after it: `requestCorelibDir` never throws (it catches
  // its own failure), so it's safe to leave running even on a build failure below, and
  // overlapping the two hides the LSP round trip behind the (usually longer) build instead
  // of adding to it.
  const corelibDirPromise = requestCorelibDir(getClient?.());
  try {
    await vscode.window.withProgress(
      {
        location: vscode.ProgressLocation.Notification,
        title: progressTitle,
        cancellable: false,
      },
      () => runCompiler(compiler, buildArgv, cwd),
    );
  } catch (error) {
    // A compiler we couldn't spawn is a setup problem, not a build failure: say how to fix
    // it, and re-resolve in case one is installed meanwhile.
    if (error instanceof CompilerMissing) {
      forgetResolvedCompiler();
      showMissingCompiler(error.compiler, "cannot debug — ");
    } else {
      void vscode.window.showErrorMessage(
        `Quilon: debug build failed: ${(error as Error).message}`,
      );
    }
    return undefined;
  }
  const corelibDir = await corelibDirPromise;
  return toLldbConfiguration({
    name: sessionName,
    program: output,
    args: programArgs,
    cwd,
    formatterPath: formatterPath(context),
    sourceFile,
    corelibDir,
  }) as vscode.DebugConfiguration;
}

/**
 * Turns a `type: "quilon"` launch into a CodeLLDB launch: it builds the source
 * with debug info, then returns the equivalent `type: "lldb"` configuration for
 * VS Code to run. Returning `undefined` aborts the session (after surfacing why).
 */
class QuilonDebugConfigurationProvider implements vscode.DebugConfigurationProvider {
  private readonly inFlight = new InFlightBuilds();

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly getClient: () => LanguageClient | undefined,
  ) {}

  provideDebugConfigurations(): vscode.DebugConfiguration[] {
    return [defaultDebugConfiguration()];
  }

  async resolveDebugConfigurationWithSubstitutedVariables(
    folder: vscode.WorkspaceFolder | undefined,
    config: vscode.DebugConfiguration,
  ): Promise<vscode.DebugConfiguration | undefined> {
    // A bare F5 with no launch.json hands us an empty config; adopt the default
    // identity but leave `program` unset so the active-editor fallback below runs.
    if (!config.type && !config.request && !config.name) {
      const { type, request, name } = defaultDebugConfiguration();
      config = { ...config, type, request, name };
    }

    const file = resolveSourceFile(config);
    if (!file || !/\.qn$/i.test(file)) {
      void vscode.window.showErrorMessage("Quilon: no active .qn file to debug.");
      return undefined;
    }
    const base = path.basename(file);

    // Refuse a second ▶ Debug for the same file while the first is still
    // starting, so an impatient re-click can't kick off a duplicate build.
    if (!this.inFlight.tryAcquire(file)) {
      void vscode.window.showInformationMessage(
        `Quilon: a debug build is already running for ${base}…`,
      );
      return undefined;
    }

    try {
      if (!(await saveIfDirty(file))) {
        return undefined;
      }

      const cwd =
        folder?.uri.fsPath ??
        vscode.workspace.getWorkspaceFolder(vscode.Uri.file(file))?.uri.fsPath;
      const output = tempBinaryPath(file);
      const compiler = resolvedQuilonCompiler();
      const programArgs = Array.isArray(config.args) ? (config.args as string[]) : [];

      return await buildDebuggable(
        this.context,
        compiler,
        buildArgs(compiler.baseArgs, file, output),
        output,
        cwd,
        `Quilon: building ${base} for debug…`,
        typeof config.name === "string" ? config.name : "Quilon Debug",
        { programArgs, sourceFile: file, getClient: this.getClient },
      );
    } finally {
      this.inFlight.release(file);
    }
  }
}

/** The default launch config we contribute and start from the Debug CodeLens. */
function defaultDebugConfiguration(): vscode.DebugConfiguration {
  return {
    type: "quilon",
    request: "launch",
    name: "Quilon: Debug current file",
    program: "${file}",
    args: [],
  };
}

/**
 * Build one test suite or case into a debuggable native executable (`quilon test <file>
 * --only <testPath> --binary <out>`) and launch it under CodeLLDB — the `quilon.debugTests`
 * command the language server's 🐞 Debug suite / 🐞 Debug case lenses invoke, with the same
 * `[filePath, testPath]` arguments the ▶ Run suite / ▶ Run case lenses' `quilon.runTests`
 * takes.
 */
async function debugTests(
  context: vscode.ExtensionContext,
  getClient: () => LanguageClient | undefined,
  file: string,
  testPath: string,
): Promise<void> {
  if (!(await saveIfDirty(file))) {
    return;
  }

  const folder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(file));
  const cwd = folder?.uri.fsPath ?? path.dirname(file);
  const output = tempBinaryPath(file);
  const compiler = resolvedQuilonCompiler();
  const base = path.basename(file);

  const config = await buildDebuggable(
    context,
    compiler,
    testBuildArgs(compiler.baseArgs, file, testPath, output),
    output,
    cwd,
    `Quilon: building ${base} for debug…`,
    `Quilon: Debug ${testPath}`,
    { sourceFile: file, getClient },
  );
  if (config) {
    await vscode.debug.startDebugging(folder, config);
  }
}

/** Whether `program` is one of the temp debug binaries we built, so it's safe to delete. */
function isOurTempBinary(program: unknown): program is string {
  return (
    typeof program === "string" &&
    path.dirname(program) === os.tmpdir() &&
    path.basename(program).startsWith("quilon-debug-")
  );
}

/**
 * Register the debug provider and the `quilon.debug` command (used by the CodeLens).
 * `getClient` reads the running language client lazily — it starts asynchronously and may
 * restart later — so `quilon/corelibDir` (the corelib source map) always asks whichever
 * client is up when a debug session actually starts.
 */
export function registerDebug(
  context: vscode.ExtensionContext,
  getClient: () => LanguageClient | undefined,
): void {
  context.subscriptions.push(
    vscode.debug.registerDebugConfigurationProvider(
      "quilon",
      new QuilonDebugConfigurationProvider(context, getClient),
    ),
    // Delete the temp binary once its session ends, so builds don't pile up.
    vscode.debug.onDidTerminateDebugSession((session) => {
      const program = session.configuration.program;
      if (isOurTempBinary(program)) {
        fs.rm(program, { force: true }, () => {});
      }
    }),
    vscode.commands.registerCommand("quilon.debug", () => {
      // `${file}` resolves to the active editor; the provider owns validation
      // and the "no active .qn file" error, so this stays a thin trigger.
      const doc = vscode.window.activeTextEditor?.document;
      const folder = doc ? vscode.workspace.getWorkspaceFolder(doc.uri) : undefined;
      void vscode.debug.startDebugging(folder, defaultDebugConfiguration());
    }),
    vscode.commands.registerCommand("quilon.debugTests", (filePath?: string, testPath?: string) => {
      if (typeof filePath !== "string" || typeof testPath !== "string") {
        return;
      }
      void debugTests(context, getClient, filePath, testPath);
    }),
  );
}
