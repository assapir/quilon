import { defineConfig } from "rolldown";

export default defineConfig({
  input: "src/extension.ts",
  // Provided by the extension host at runtime.
  external: ["vscode"],
  platform: "node",
  output: {
    file: "out/extension.js",
    format: "cjs",
    sourcemap: true,
    minify: false,
  },
});
