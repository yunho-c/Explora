import { createRequire } from "node:module";
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import tailwindcss from "@tailwindcss/vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig, type Plugin } from "vite";

const host = process.env.TAURI_DEV_HOST;
const require = createRequire(import.meta.url);
const pdfjsRoot = path.dirname(require.resolve("pdfjs-dist/package.json"));
const pdfjsAssetGroups = ["cmaps", "iccs", "standard_fonts", "wasm"] as const;

const pdfjsAssets = (): Plugin => {
  let command: "build" | "serve" = "serve";

  return {
    name: "explora-pdfjs-assets",
    configResolved(config) {
      command = config.command;
    },
    configureServer(server) {
      server.middlewares.use("/pdfjs", (request, response, next) => {
        let pathname: string;
        try {
          pathname = decodeURIComponent(request.url?.split("?", 1)[0] ?? "");
        } catch {
          next();
          return;
        }
        const match =
          /^\/(cmaps|iccs|standard_fonts|wasm)\/([A-Za-z0-9._-]+)$/.exec(
            pathname,
          );
        if (!match) {
          next();
          return;
        }
        const file = path.join(pdfjsRoot, match[1], match[2]);
        try {
          response.statusCode = 200;
          response.setHeader("Cache-Control", "no-store");
          response.setHeader(
            "Content-Type",
            path.extname(file) === ".wasm"
              ? "application/wasm"
              : path.extname(file) === ".js"
                ? "text/javascript; charset=utf-8"
                : "application/octet-stream",
          );
          response.end(readFileSync(file));
        } catch {
          next();
        }
      });
    },
    buildStart() {
      if (command !== "build") return;
      for (const group of pdfjsAssetGroups) {
        const directory = path.join(pdfjsRoot, group);
        for (const file of readdirSync(directory, { withFileTypes: true })) {
          if (!file.isFile()) continue;
          this.emitFile({
            type: "asset",
            fileName: `pdfjs/${group}/${file.name}`,
            source: readFileSync(path.join(directory, file.name)),
          });
        }
      }
    },
  };
};

export default defineConfig({
  plugins: [pdfjsAssets(), tailwindcss(), svelte()],
  resolve: {
    alias: {
      $lib: path.resolve("./src/lib"),
    },
  },
  clearScreen: false,
  server: {
    port: 6748,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 6749,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
