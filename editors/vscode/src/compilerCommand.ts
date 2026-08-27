// Resolving *how* to invoke the Quilon compiler, kept free of any `vscode`
// import so it runs under plain Node and is unit-testable.
//
// `quilon.command` defaults to a bare `quilon`, which only works when the
// extension host inherited a PATH containing it. A GUI-launched editor often
// does not (PATH additions like `~/.cargo/bin` usually come from a shell rc
// file), so a working toolchain still produced "could not run \"quilon\"". When
// the setting is left at its default we therefore look for the compiler
// ourselves: PATH, the usual install directories, a checkout's `target/`
// binaries, and finally `cargo run --` in a checkout of the compiler repo.
//
// A configured `quilon.command` is used verbatim — the search exists to make the
// default work, never to second-guess a setting. The one exception is a bare
// `quilon`, which is what the default already is and what fails in the first
// place, so it searches too.

import * as path from "node:path";
import { splitCommand } from "./debugConfig";

/** What the resolver needs to know about the machine; injected so tests stay pure. */
export interface CompilerProbe {
  /** The `quilon.command` value the user explicitly set, if any. */
  configured?: string;
  /** `PATH` as the extension host sees it. */
  path?: string;
  /** `PATHEXT` — the extensions an argv-0 may take on Windows. */
  pathExt?: string;
  /** The user's home directory. */
  home?: string;
  /** Open workspace folder paths, in order. */
  workspaceFolders?: readonly string[];
  /** `"win32"` selects Windows path/extension conventions. */
  platform?: string;
  /** Whether `file` exists and can be executed. */
  isExecutable(file: string): boolean;
  /** The text of `file`, or undefined if it can't be read. */
  readText(file: string): string | undefined;
}

/** Where a resolved invocation came from, for the "how did I get this" message. */
export type CompilerOrigin =
  | "configured"
  | "path"
  | "install-dir"
  | "checkout-binary"
  | "cargo"
  | "fallback";

/** A compiler invocation: the executable, its leading arguments, and how it was arrived at. */
export interface ResolvedCompiler {
  /** The executable to spawn, e.g. `/home/u/.cargo/bin/quilon` or `cargo`. */
  exe: string;
  /** Arguments that precede the subcommand, e.g. `["run", "--quiet", "--"]`. */
  baseArgs: string[];
  origin: CompilerOrigin;
}

/** Join path segments the way the probed platform does, whatever this host is. */
function joinPath(probe: CompilerProbe, ...parts: string[]): string {
  return (probe.platform === "win32" ? path.win32 : path.posix).join(...parts);
}

/** Install directories a compiler commonly lands in but a GUI PATH commonly misses. */
function installDirs(probe: CompilerProbe): string[] {
  const dirs = ["/usr/local/bin", "/opt/homebrew/bin"];
  if (probe.home !== undefined && probe.home.length > 0) {
    dirs.unshift(
      joinPath(probe, probe.home, ".cargo", "bin"),
      joinPath(probe, probe.home, ".local", "bin"),
    );
  }
  return dirs;
}

/** The file names `name` may have as an executable (`quilon.exe` etc. on Windows). */
function executableNames(name: string, probe: CompilerProbe): string[] {
  if (probe.platform !== "win32") {
    return [name];
  }
  const exts = (probe.pathExt ?? ".COM;.EXE;.BAT;.CMD")
    .split(";")
    .map((e) => e.trim())
    .filter((e) => e.length > 0);
  return [name, ...exts.map((ext) => `${name}${ext}`)];
}

/** The first `dir` in `dirs` holding an executable `name`, as a full path. */
function findIn(dirs: readonly string[], name: string, probe: CompilerProbe): string | undefined {
  for (const dir of dirs) {
    if (dir.length === 0) {
      continue;
    }
    for (const candidate of executableNames(name, probe)) {
      const full = joinPath(probe, dir, candidate);
      if (probe.isExecutable(full)) {
        return full;
      }
    }
  }
  return undefined;
}

/** The directories on `PATH`, split on the platform's separator. */
function pathDirs(probe: CompilerProbe): string[] {
  const separator = probe.platform === "win32" ? ";" : ":";
  return (probe.path ?? "").split(separator).filter((dir) => dir.length > 0);
}

/** Whether `folder` is a checkout of the compiler repo (its Cargo.toml names the crate). */
function isCompilerCheckout(folder: string, probe: CompilerProbe): boolean {
  const manifest = probe.readText(joinPath(probe, folder, "Cargo.toml"));
  return manifest !== undefined && /^\s*name\s*=\s*"quilon"/m.test(manifest);
}

/** A `target/{release,debug}/quilon` built in one of the open folders. */
function checkoutBinary(probe: CompilerProbe): string | undefined {
  for (const folder of probe.workspaceFolders ?? []) {
    const built = findIn(
      [joinPath(probe, folder, "target", "release"), joinPath(probe, folder, "target", "debug")],
      "quilon",
      probe,
    );
    if (built !== undefined) {
      return built;
    }
  }
  return undefined;
}

/** `cargo run --quiet --` when an open folder is a compiler checkout and cargo is available. */
function cargoInvocation(probe: CompilerProbe): ResolvedCompiler | undefined {
  const checkout = (probe.workspaceFolders ?? []).some((folder) =>
    isCompilerCheckout(folder, probe),
  );
  if (!checkout) {
    return undefined;
  }
  const cargo = findIn([...pathDirs(probe), ...installDirs(probe)], "cargo", probe);
  if (cargo === undefined) {
    return undefined;
  }
  // `--quiet` keeps cargo's progress lines out of the compiler output the
  // diagnostics parser reads.
  return { exe: cargo, baseArgs: ["run", "--quiet", "--"], origin: "cargo" };
}

/** Whether an invocation is just the compiler's own name, carrying no path and no arguments. */
function isBareDefault(invocation: { exe: string; baseArgs: string[] }): boolean {
  return (
    invocation.baseArgs.length === 0 &&
    /^quilon(\.exe)?$/i.test(invocation.exe) &&
    !/[/\\]/.test(invocation.exe)
  );
}

/**
 * How to invoke the compiler: the configured command when there is a meaningful
 * one, else the first of PATH, the common install directories, a checkout's built
 * binary, and `cargo run --` in a checkout. Falls back to a bare `quilon` so the
 * caller still has something to run (and a name to report) when nothing matched.
 */
export function resolveCompilerCommand(probe: CompilerProbe): ResolvedCompiler {
  const configured = probe.configured?.trim() ?? "";
  if (configured.length > 0) {
    const invocation = splitCommand(configured);
    // A bare `quilon` says nothing the default doesn't, and is exactly the value
    // that leaves a GUI-launched host with nothing to run — so search anyway.
    if (!isBareDefault(invocation)) {
      return { ...invocation, origin: "configured" };
    }
  }

  const onPath = findIn(pathDirs(probe), "quilon", probe);
  if (onPath !== undefined) {
    return { exe: onPath, baseArgs: [], origin: "path" };
  }

  const installed = findIn(installDirs(probe), "quilon", probe);
  if (installed !== undefined) {
    return { exe: installed, baseArgs: [], origin: "install-dir" };
  }

  const built = checkoutBinary(probe);
  if (built !== undefined) {
    return { exe: built, baseArgs: [], origin: "checkout-binary" };
  }

  const cargo = cargoInvocation(probe);
  if (cargo !== undefined) {
    return cargo;
  }

  return { exe: "quilon", baseArgs: [], origin: "fallback" };
}

/** Why a resolved invocation could not be spawned, phrased for the user's next step. */
export function missingCompilerMessage(resolved: ResolvedCompiler): string {
  if (resolved.origin === "configured") {
    return `could not run "${resolved.exe}" from the "quilon.command" setting. Point it at your compiler (e.g. "cargo run --").`;
  }
  return `no Quilon compiler found — looked on PATH, in the usual install directories, and in this workspace. Install one with "cargo install --path ." or set "quilon.command" (e.g. "cargo run --").`;
}

/**
 * A resolved invocation as one shell command line, for the commands that type
 * into the integrated terminal. Only whitespace needs quoting: everything here
 * is either a path we found or the user's own setting.
 */
export function shellCommand(resolved: ResolvedCompiler): string {
  return [resolved.exe, ...resolved.baseArgs].map(quoteIfSpaced).join(" ");
}

/** `part` in double quotes when it contains whitespace, otherwise as-is. */
function quoteIfSpaced(part: string): string {
  return /\s/.test(part) ? `"${part}"` : part;
}
