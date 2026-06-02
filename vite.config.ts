import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;
// @ts-expect-error process is a nodejs global
const isTest = process.env.VITEST === "true";

export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  // Im Testlauf den Client-Build von solid-js verwenden.
  resolve: isTest ? { conditions: ["development", "browser"] } : undefined,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.{test,spec}.ts"],
  },
});
