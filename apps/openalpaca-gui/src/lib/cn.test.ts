import { describe, expect, it } from "vitest";

import { cn } from "./cn";

describe("cn", () => {
  it("keeps a colour when a custom font size is also applied", () => {
    // Regression guard: stock tailwind-merge classifies `text-md-plus` as a
    // colour and drops `text-ink`.
    expect(cn("text-ink", "text-md-plus")).toBe("text-ink text-md-plus");
  });

  it("still resolves conflicts inside the extended font-size group", () => {
    expect(cn("text-sm-plus", "text-base-plus")).toBe("text-base-plus");
  });

  it("keeps a shadow token from eating a colour", () => {
    expect(cn("text-ink", "shadow-card")).toBe("text-ink shadow-card");
  });

  it("lets a later class win a standard conflict", () => {
    expect(cn("px-[10px]", "px-[16px]")).toBe("px-[16px]");
  });

  it("drops falsy input", () => {
    expect(cn("a", false && "b", null, undefined, ["c"])).toBe("a c");
  });
});
