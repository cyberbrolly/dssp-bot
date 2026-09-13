import { describe, expect, it } from "vitest";

import { parsePortalMatches } from "../../manifest";

describe("portal manifest matches", () => {
  it("injects the content script into the live DSSP origin by default", () => {
    expect(parsePortalMatches(undefined)).toEqual([
      "https://dssp.frsc.gov.ng/*",
    ]);
  });
});
