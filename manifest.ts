import { defineManifest } from "@crxjs/vite-plugin";

const FALLBACK_PORTAL_MATCH = "https://portal.dssp.invalid/*";

export function parsePortalMatches(value: string | undefined): string[] {
  const patterns = (value ?? "")
    .split(",")
    .map((pattern) => pattern.trim())
    .filter((pattern) => pattern.length > 0);

  return patterns.length > 0 ? patterns : [FALLBACK_PORTAL_MATCH];
}

export function createManifest(portalMatches: string[]) {
  return defineManifest({
    manifest_version: 3,

    name: "DSSP Training Logger",

    short_name: "DSSP",

    version: "0.1.0",

    description: "Automates training log submissions for DSSP instructors.",

    icons: {
      16: "icons/icon16.png",
      32: "icons/icon32.png",
      48: "icons/icon48.png",
      128: "icons/icon128.png",
    },

    action: {
      default_popup: "src/popup/index.html",
      default_title: "DSSP Training Logger",
    },

     background: {
      service_worker: "src/background/service-worker.ts",
      type: "module",
    },

    content_scripts: [
      {
        matches: portalMatches,
        js: ["src/content/main-world.ts"],
        run_at: "document_start",
        world: "MAIN",
      },
      {
        matches: portalMatches,
        js: ["src/content/content-script.ts"],
        run_at: "document_idle",
        world: "ISOLATED",
      },
    ],

    permissions: ["storage", "tabs", "activeTab"],

    host_permissions: portalMatches,
  });
}
