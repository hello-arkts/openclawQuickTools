import { defineConfig } from "vite";

function tauriFix() {
  return {
    name: "tauri-fix-crossorigin",
    transformIndexHtml(html: string) {
      return html.replace(/crossorigin/g, "");
    },
  };
}

export default defineConfig({
  clearScreen: false,
  base: "",
  server: { port: 1420, strictPort: true },
  plugins: [tauriFix()],
  build: {
    // Tauri WebView2 can't handle code-split chunks well.
    // Force everything into a single file.
    modulePreload: false,
    cssCodeSplit: false,
    rollupOptions: {
      output: {
        manualChunks: undefined,
        inlineDynamicImports: true,
      },
    },
  },
});
