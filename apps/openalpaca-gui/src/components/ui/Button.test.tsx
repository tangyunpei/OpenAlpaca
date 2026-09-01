import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Button, button, chipVariant, pinVariant } from "./Button";

describe("Button", () => {
  it("defaults to type=button so it never submits a form by accident", () => {
    render(<Button>Cancel</Button>);
    expect(screen.getByRole("button")).toHaveAttribute("type", "button");
  });

  it("selects the requested catalogue row", () => {
    const classes = button({ variant: "primaryMd" });
    // §3.35 row 2: ink fill, 7px radius, 9px/16px padding, 13px/600.
    expect(classes).toContain("bg-ink");
    expect(classes).toContain("rounded-lg");
    expect(classes).toContain("px-[16px]");
    expect(classes).toContain("py-[9px]");
    expect(classes).toContain("text-md");
    expect(classes).toContain("font-semibold");
    expect(classes).toContain("hover:bg-ink-hover");
  });

  it("falls back to the secondary sm row", () => {
    expect(button()).toBe(button({ variant: "secondarySm" }));
  });

  it("lets a caller override one property without losing the variant", () => {
    render(
      <Button variant="pinOff" className="px-[10px] py-[5px] text-sm-plus">
        ☆ Pin
      </Button>,
    );
    const classes = screen.getByRole("button").className;
    // §3.31's larger pin: padding and size swapped, colours kept.
    expect(classes).toContain("px-[10px]");
    expect(classes).not.toContain("px-[8px]");
    expect(classes).toContain("text-sm-plus");
    expect(classes).not.toContain("text-xs-plus");
    expect(classes).toContain("border-line");
  });

  it("maps the two boolean pairs", () => {
    expect(pinVariant(true)).toBe("pinOn");
    expect(pinVariant(false)).toBe("pinOff");
    expect(chipVariant(true)).toBe("chipOn");
    expect(chipVariant(false)).toBe("chipOff");
  });

  it("keeps the browser's default line-height on every row", () => {
    // The theme pairs a line-height with each font size, and tailwind-merge
    // lets a font size delete `leading-*`; the design declares none, so every
    // row has to carry `leading-[normal]` after its own size.
    const variants = [
      "primaryBlock",
      "primaryMd",
      "primarySm",
      "secondaryMd",
      "secondarySm",
      "secondaryXs",
      "ghostSm",
      "ghostXs",
      "ghost2xs",
      "outlineRaised",
      "dangerGhost",
      "bareLink",
      "iconGlyph",
      "pinOff",
      "pinOn",
      "chipOff",
      "chipOn",
    ] as const;
    expect(variants).toHaveLength(17);
    for (const variant of variants) {
      expect(button({ variant })).toContain("leading-[normal]");
    }
  });

  it("carries a focus-visible ring, which the design lacks", () => {
    expect(button({ variant: "ghostSm" })).toContain(
      "focus-visible:outline-blue",
    );
  });
});
