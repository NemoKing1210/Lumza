import { describe, expect, it } from "vitest";
import { sameMode } from "./ipc";

describe("sameMode", () => {
  it("compares type and key", () => {
    expect(sameMode({ type: "extend" }, { type: "extend" })).toBe(true);
    expect(sameMode({ type: "only", key: "a" }, { type: "only", key: "a" })).toBe(true);
    expect(sameMode({ type: "only", key: "a" }, { type: "only", key: "b" })).toBe(false);
    expect(sameMode({ type: "duplicate" }, { type: "extend" })).toBe(false);
  });

  it("marks nothing when the current mode is unknown", () => {
    expect(sameMode(null, { type: "extend" })).toBe(false);
  });
});
