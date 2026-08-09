import { defineConfig } from "vitest/config";
import { loadEnv } from "vite";
import { crx } from "@crxjs/vite-plugin";
import { createManifest, parsePortalMatches } from "./manifest.ts";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "VITE_");

  const manifest = createManifest(
    parsePortalMatches(env["VITE_PORTAL_MATCHES"]),
  );

  return {
    plugins: [crx({ manifest })],
    test: {
      environment: "node",
      include: ["tests/**/*.test.ts"],
    },
  };
});
