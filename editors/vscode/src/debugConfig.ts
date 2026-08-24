// Pure helpers for the Quilon debug integration, kept free of any `vscode`
// import so they run under plain Node and are unit-testable.
//
// The debug flow is: build the active `.qn` with `<command> build --debug
// <file> -o <tmpbin>`, then hand `<tmpbin>` to CodeLLDB (debug type "lldb").
// Breakpoints placed in the `.qn` source are hit through the DWARF line table
// the compiler's `--debug` build emits. These functions produce the compiler
// argv and the resolved CodeLLDB configuration; the `vscode`-facing provider in
// `debug.ts` wires them to the editor.

import * as os from "node:os";
import * as path from "node:path";

/**
 * Split the `quilon.command` setting into an executable plus leading arguments,
 * so a value like `cargo run --` runs `cargo` with `["run", "--", ...]`. A
 * simple whitespace split is sufficient: paths with spaces should be configured
 * via PATH or a wrapper rather than embedded here. Shared with the diagnostics
 * path so both invoke the compiler the same way.
 */
export function splitCommand(command: string): { exe: string; baseArgs: string[] } {
  const trimmed = command.trim();
  if (trimmed.length === 0) {
    return { exe: "quilon", baseArgs: [] };
  }
  const [exe = "quilon", ...baseArgs] = trimmed.split(/\s+/);
  return { exe, baseArgs };
}

/**
 * Refuses a second debug build for a source file while the first is still in
 * flight. `tryAcquire` returns false when `key` is already held; always
 * `release` it in a `finally` so the flag can never wedge.
 */
export class InFlightBuilds {
  private readonly active = new Set<string>();

  tryAcquire(key: string): boolean {
    if (this.active.has(key)) {
      return false;
    }
    this.active.add(key);
    return true;
  }

  release(key: string): void {
    this.active.delete(key);
  }
}

/** The first non-blank line of `output`, trimmed — used to distill a one-line reason from compiler stderr. */
export function firstNonEmptyLine(output: string): string | undefined {
  return output
    .split(/\r?\n/)
    .find((l) => l.trim().length > 0)
    ?.trim();
}

/**
 * The compiler argv that builds `file` into `output` with DWARF line info.
 * `baseArgs` are the leading args from `splitCommand` (e.g. `["run", "--"]` for
 * the `cargo run --` setting). `--debug` is what makes breakpoints/stepping
 * resolve against `.qn` source lines.
 */
export function buildArgs(baseArgs: string[], file: string, output: string): string[] {
  return [...baseArgs, "build", "--debug", file, "-o", output];
}

/**
 * A collision-resistant temp path for the debug executable. The name embeds the
 * source's base name (so it is recognizable in the debugger UI) plus the process
 * id and a caller-supplied uniquifier. `tmpDir` is injectable for tests.
 */
export function tempBinaryPath(
  file: string,
  uniquifier: string | number = Date.now(),
  tmpDir: string = os.tmpdir(),
): string {
  const base = path.basename(file).replace(/\.(qn|ql)$/i, "") || "program";
  return path.join(tmpDir, `quilon-debug-${base}-${process.pid}-${uniquifier}`);
}

/** Inputs for a resolved CodeLLDB launch configuration. */
export interface LldbConfigInput {
  /** Display name shown in the debug UI. */
  name: string;
  /** The freshly built native executable to launch. */
  program: string;
  /** Arguments forwarded to the program (its `^` `args`). */
  args?: string[];
  /** Working directory for the debuggee. */
  cwd?: string;
  /**
   * Path to the lldb Python formatter file. When set, it is imported at debug
   * start so Quilon values render nicely (see `formatters/quilon.py`).
   */
  formatterPath?: string;
}

/**
 * Build the CodeLLDB (`type: "lldb"`) launch configuration for a Quilon debug
 * session. The Quilon `DebugConfigurationProvider` returns this in place of the
 * `type: "quilon"` config it received, so VS Code dispatches to CodeLLDB.
 */
export function toLldbConfiguration(input: LldbConfigInput): Record<string, unknown> {
  const config: Record<string, unknown> = {
    type: "lldb",
    request: "launch",
    name: input.name,
    program: input.program,
    args: input.args ?? [],
    cwd: input.cwd ?? "${workspaceFolder}",
    // Route program I/O to the shared Debug Console rather than a per-session
    // integrated terminal, so terminals don't pile up run after run.
    terminal: "console",
  };
  if (input.formatterPath !== undefined && input.formatterPath.length > 0) {
    // Load the Quilon value formatters into the lldb session before the target
    // runs. Quote the path so spaces in the extension install path survive.
    config["initCommands"] = [`command script import "${input.formatterPath}"`];
  }
  return config;
}
