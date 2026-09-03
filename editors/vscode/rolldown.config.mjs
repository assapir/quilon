// Bundles src/extension.ts (already type-checked by `tsc`, see the `compile`
// script) into a single out/extension.js for packaging, so `vsce package`
// ships one file instead of the whole node_modules tree. `vscode` is provided
// by the extension host at runtime, so it stays external rather than bundled.
import { defineConfig } from "rolldown";

export default defineConfig({
  input: "src/extension.ts",
  external: ["vscode"],
  platform: "node",
  output: {
    file: "out/extension.js",
    format: "cjs",
    sourcemap: true,
    minify: false,
  },
});
