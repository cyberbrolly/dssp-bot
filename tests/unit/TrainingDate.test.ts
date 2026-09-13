import { describe, expect, it } from "vitest";

import { formatTrainingDate } from "../../src/core/shared/trainingDate";

describe("formatTrainingDate", () => {
  it.each([
    ["2026-08-14", "2026-08-14"],
    ["2026/08/14", "2026-08-14"],
    ["14/08/2026", "2026-08-14"],
    ["14-08-2026", "2026-08-14"],
  ])("formats %s as strict YYYY-MM-DD", (input, expected) => {
    expect(formatTrainingDate(input)).toBe(expected);
    expect(expected).toMatch(/^\d{4}-\d{2}-\d{2}$/);
  });

  it.each(["", "08/14/2026", "2026-02-30", "14.08.2026"])(
    "rejects invalid date %s",
    (input) => {
      expect(() => formatTrainingDate(input)).toThrow();
    },
  );
});
