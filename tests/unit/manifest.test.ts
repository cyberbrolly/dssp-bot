import { describe, expect, it } from "vitest";
import { createManifest } from "../../manifest";

describe("thin daemon client manifest", () => {
  it("permits loopback only and never activates legacy portal scripts", () => {
    const manifest = createManifest(["https://arbitrary.example/*"]);
    expect(manifest).toMatchObject({
      permissions: ["storage"],
      host_permissions: ["http://127.0.0.1/*"],
      background: {
        service_worker: "src/background/service-worker.ts",
        type: "module",
      },
    });
    expect(manifest).not.toHaveProperty("content_scripts");
  });
});
