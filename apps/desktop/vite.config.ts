import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { readFileSync } from "node:fs";

// tauri.conf.json is the one place the shipped version is set, so read it from
// there rather than package.json, which has drifted before. A crash report
// without a version in it is much harder to act on.
const version = JSON.parse(
  readFileSync(new URL("./src-tauri/tauri.conf.json", import.meta.url), "utf-8")
).version as string;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  define: { __APP_VERSION__: JSON.stringify(version) },
});
