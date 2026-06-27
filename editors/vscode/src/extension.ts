// Minimal Quilon extension: registers two commands that run the Quilon
// compiler on the active .ql file inside an integrated terminal.
//
// NOTE: real diagnostics (squiggles) are intentionally not wired up. The
// compiler currently reports errors using *byte offsets* (e.g.
// "Type mismatch at Span { start: 42, end: 47 }"), not line:column, so the
// output cannot be reliably mapped to editor ranges with a problem matcher.
// See README.md ("Diagnostics & debugging").

import * as vscode from "vscode";

function runOnActiveFile(subcommand: string): void {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "quilon") {
    void vscode.window.showErrorMessage("Quilon: no active .ql file.");
    return;
  }
  const document = editor.document;
  // Save first so the compiler sees the latest content.
  void document.save().then(() => {
    const cmd = vscode.workspace
      .getConfiguration("quilon")
      .get<string>("command", "quilon");
    const file = document.fileName;
    const term =
      vscode.window.terminals.find((t) => t.name === "Quilon") ??
      vscode.window.createTerminal("Quilon");
    term.show();
    // Quote the path to tolerate spaces.
    term.sendText(`${cmd} ${subcommand} "${file}"`);
  });
}

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("quilon.check", () =>
      runOnActiveFile("check")
    ),
    vscode.commands.registerCommand("quilon.run", () => runOnActiveFile("run"))
  );
}

export function deactivate(): void {}
