import { describe, expect, it } from "vitest";

import { tv } from "./tv";

/**
 * Regression guard for the whole primitive set: an unconfigured `tv` files
 * `text-base-plus` under `text-color`, so the colour variant would delete the
 * size variant and every fractional size in the design would silently fall back
 * to the browser default.
 */
describe("configured tv", () => {
  const sample = tv({
    base: "font-sans",
    variants: {
      size: { library: "text-base-plus", panel: "text-sm-plus" },
      active: { true: "text-ink", false: "text-muted-fg" },
    },
  });

  it("keeps a fractional font size alongside a colour", () => {
    const classes = sample({ size: "library", active: true });
    expect(classes).toContain("text-base-plus");
    expect(classes).toContain("text-ink");
  });

  it("still resolves a real size conflict", () => {
    expect(sample({ size: "panel", active: false })).not.toContain(
      "text-base-plus",
    );
  });
});
