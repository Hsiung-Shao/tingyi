import { defineConfig } from 'vite';

// Tauri 開發模式固定連 1430(tauri.conf.json 的 devUrl)
export default defineConfig({
  clearScreen: false,
  server: { port: 1430, strictPort: true },
  build: { target: 'chrome120', outDir: 'dist', emptyOutDir: true },
});
