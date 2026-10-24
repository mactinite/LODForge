import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Builds into ../dist, which Tauri serves (tauri.conf.json frontendDist).
export default defineConfig({
  plugins: [react()],
  base: "./",
  build: { outDir: "../dist", emptyOutDir: true, chunkSizeWarningLimit: 2000 },
  server: { port: 5173, strictPort: true },
});
