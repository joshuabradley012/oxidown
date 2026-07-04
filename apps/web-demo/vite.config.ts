import { defineConfig } from "vite";

export default defineConfig({
  // A single instance of the CM6 core packages is required — duplicated
  // @codemirror/state instances break extensions at runtime.
  resolve: {
    dedupe: ["@codemirror/state", "@codemirror/view"],
  },
  build: {
    // top-level await in main.ts (core selection)
    target: "es2022",
  },
});
