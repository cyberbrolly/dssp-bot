import { defineManifest } from "@crxjs/vite-plugin";

export default defineManifest({
  manifest_version: 3,

  name: "DSSP Training Logger",

  short_name: "DSSP",

  version: "0.1.0",

  description:
    "Automates training log submissions for DSSP instructors.",

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
    service_worker: "src/background/index.ts",
    type: "module",
  },

  content_scripts: [
    {
      matches: ["<all_urls>"],
      js: ["src/content/index.ts"],
      run_at: "document_idle",
    },
  ],

  permissions: [
    "storage",
    "tabs",
    "activeTab",
    "scripting",
  ],

  host_permissions: ["<all_urls>"],

  web_accessible_resources: [
    {
      resources: ["assets/*"],
      matches: ["<all_urls>"],
    },
  ],
});