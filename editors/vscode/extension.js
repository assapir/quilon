// Minimal Quilon extension: registers two commands that run the Quilon
// compiler on the active .ql file inside an integrated terminal.
//
// NOTE: real diagnostics (squiggles) are intentionally not wired up. The
// compiler currently reports errors using *byte offsets* (e.g.
// "Type mismatch at Span { start: 42, end: 47 }"), not line:column, so the
// output cannot be reliably mapped to editor ranges with a problem matcher.
// See README.md ("Diagnostics & debugging").

const vscode = require("vscode");

/** @param {string} subcommand */
function runOnActiveFile(subcommand) {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "quilon") {
    vscode.window.showErrorMessage("Quilon: no active .ql file.");
    return;
  }
  // Save first so the compiler sees the latest content.
  editor.document.save().then(() => {
    const cmd = vscode.workspace
      .getConfiguration("quilon")
      .get("command", "quilon");
    const file = editor.document.fileName;
    const term =
      vscode.window.terminals.find((t) => t.name === "Quilon") ||
      vscode.window.createTerminal("Quilon");
    term.show();
    // Quote the path to tolerate spaces.
    term.sendText(`${cmd} ${subcommand} "${file}"`);
  });
}

function activate(context) {
  context.subscriptions.push(
    vscode.commands.registerCommand("quilon.check", () =>
      runOnActiveFile("check")
    ),
    vscode.commands.registerCommand("quilon.run", () => runOnActiveFile("run"))
  );
}

function deactivate() {}

module.exports = { activate, deactivate };
