// Quilon debug integration.
//
// Debugging is delegated to CodeLLDB (`vadimcn.vscode-lldb`, a declared
// extension dependency). We contribute a `quilon` debug type whose
// DebugConfigurationProvider, when a session starts, builds the active `.qn`
// with `<command> build --debug <file> -o <tmpbin>` and then resolves into a
// CodeLLDB (`type: "lldb"`) launch of that binary. Breakpoints set in the `.qn`
// source are hit through the DWARF line table the `--debug` build emits.
//
// Value rendering (Text as a string, `[]T` as an indexed list) is provided by
// the lldb formatter we import against the compiler's DWARF types — see
// `formatters/quilon.py`.

import { execFile } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import {
  buildArgs,
  firstNonEmptyLine,
  InFlightBuilds,
  splitCommand,
  tempBinaryPath,
  toLldbConfiguration,
} from "./debugConfig";
import { quilonCommand } from "./extension";

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
async function saveIfDirty(file: string): Promise<boolean> {
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

/** Build `file` into `output` with DWARF line info; reject with the compiler's message on failure. */
function buildDebugBinary(file: string, output: string, cwd: string | undefined): Promise<void> {
  const { exe, baseArgs } = splitCommand(quilonCommand());
  return new Promise((resolve, reject) => {
    execFile(exe, buildArgs(baseArgs, file, output), { cwd }, (error, _stdout, stderr) => {
      if (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") {
          reject(
            new Error(
              `could not run "${exe}". Set "quilon.command" to your compiler (e.g. "cargo run --").`,
            ),
          );
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
 * Turns a `type: "quilon"` launch into a CodeLLDB launch: it builds the source
 * with debug info, then returns the equivalent `type: "lldb"` configuration for
 * VS Code to run. Returning `undefined` aborts the session (after surfacing why).
 */
class QuilonDebugConfigurationProvider implements vscode.DebugConfigurationProvider {
  private readonly inFlight = new InFlightBuilds();

  constructor(private readonly context: vscode.ExtensionContext) {}

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

    if (!vscode.extensions.getExtension(CODELLDB_ID)) {
      void vscode.window.showErrorMessage(
        "Quilon debugging needs the CodeLLDB extension (vadimcn.vscode-lldb). Install it and try again.",
      );
      return undefined;
    }

    const file = resolveSourceFile(config);
    // `.ql` is deprecated but still compiles for this release, so it is still debuggable.
    if (!file || !/\.(qn|ql)$/i.test(file)) {
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

      try {
        await vscode.window.withProgress(
          {
            location: vscode.ProgressLocation.Notification,
            title: `Quilon: building ${base} for debug…`,
            cancellable: false,
          },
          () => buildDebugBinary(file, output, cwd),
        );
      } catch (error) {
        void vscode.window.showErrorMessage(
          `Quilon: debug build failed: ${(error as Error).message}`,
        );
        return undefined;
      }

      const programArgs = Array.isArray(config.args) ? (config.args as string[]) : [];
      return toLldbConfiguration({
        name: typeof config.name === "string" ? config.name : "Quilon Debug",
        program: output,
        args: programArgs,
        cwd,
        formatterPath: formatterPath(this.context),
      }) as vscode.DebugConfiguration;
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

/** Whether `program` is one of the temp debug binaries we built, so it's safe to delete. */
function isOurTempBinary(program: unknown): program is string {
  return (
    typeof program === "string" &&
    path.dirname(program) === os.tmpdir() &&
    path.basename(program).startsWith("quilon-debug-")
  );
}

/** Register the debug provider and the `quilon.debug` command (used by the CodeLens). */
export function registerDebug(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.debug.registerDebugConfigurationProvider(
      "quilon",
      new QuilonDebugConfigurationProvider(context),
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
  );
}
