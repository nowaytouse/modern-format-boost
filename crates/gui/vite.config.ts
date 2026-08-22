import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";

// https://vite.dev/config/
export default defineConfig({
  base: "./",
  plugins: [
    vue(),
    {
      name: "wkwebview-file-entry",
      apply: "build",
      enforce: "post",
      transformIndexHtml: (html) =>
        html.replace(' type="module"', " defer").replaceAll(" crossorigin", ""),
    },
  ],
  define: {
    __VUE_I18N_FULL_INSTALL__: true,
    __VUE_I18N_LEGACY_API__: false,
    __INTLIFY_PROD_DEVTOOLS__: false,
  },

  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-macos/**"],
    },
  },
  build: {
    target: "esnext",
    minify: "esbuild",
    cssCodeSplit: true,
  },
  esbuild: {
    drop: ["console", "debugger"],
  },
});
