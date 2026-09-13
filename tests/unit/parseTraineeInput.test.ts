import { describe, expect, it } from "vitest";

import { parseTraineeInput } from "../../src/core/shared/parseTraineeInput";

describe("parseTraineeInput", () => {
  it.each([
    ["comma separated", "John Doe, Jane Smith, Michael Brown"],
    ["newline separated", "John Doe\nJane Smith\nMichael Brown"],
    ["mixed separators", "John Doe, Jane Smith\nMichael Brown"],
  ])("parses %s names in order", (_case, input) => {
    expect(parseTraineeInput(input)).toEqual([
      "John Doe",
      "Jane Smith",
      "Michael Brown",
    ]);
  });

  it("removes empty and accidental duplicate names", () => {
    expect(parseTraineeInput(" John Doe, ,\n john   doe\nJane Smith,"))
      .toEqual(["John Doe", "Jane Smith"]);
  });
});
