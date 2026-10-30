import { defineConfig } from "vite";

// The phone app: a second bundle, served by the desktop app to paired
// phones. Built into dist-mobile, which the Rust build embeds.
export default defineConfig({
  root: "mobile",
  base: "/",
  clearScreen: false,
  build: {
    outDir: "../dist-mobile",
    emptyOutDir: true,
    target: "safari16",
  },
});
