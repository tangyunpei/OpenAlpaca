import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Tag, toTagTone } from "./Badge";
import { FileBadge } from "./FileBadge";
import { matchesKindFilter } from "./KindFilterChip";
import { LogTag, toLogTone } from "./LogTag";
import { Tab } from "./Tab";

describe("LogTag", () => {
  it("keeps the four known tones", () => {
    expect(toLogTone("tool")).toBe("tool");
    expect(toLogTone("STEER")).toBe("steer");
    expect(toLogTone("artifact")).toBe("artifact");
    expect(toLogTone("spawn")).toBe("spawn");
  });

  it("degrades anything unknown to the neutral run tone (§3.28)", () => {
    expect(toLogTone("run")).toBe("run");
    expect(toLogTone("extension_state_changed")).toBe("run");
  });

  it("holds the 58px column that aligns the message text", () => {
    render(<LogTag value="tool" />);
    expect(screen.getByText("tool").className).toContain("w-[58px]");
  });
});

describe("Tag", () => {
  it("maps the settings tone table (§3.32)", () => {
    expect(toTagTone("unwired")).toBe("warn");
    expect(toTagTone("warn")).toBe("warn");
    expect(toTagTone("asks")).toBe("asks");
    expect(toTagTone("live")).toBe("live");
    expect(toTagTone("active")).toBe("live");
    expect(toTagTone("beta")).toBe("neutral");
  });

  it("renders the word verbatim", () => {
    render(<Tag value="unwired" />);
    expect(screen.getByText("unwired")).toBeInTheDocument();
  });
});

describe("KindFilterChip", () => {
  it("treats All as a wildcard and Media as two kinds (§3.29)", () => {
    expect(matchesKindFilter("All", "term")).toBe(true);
    expect(matchesKindFilter("Media", "image")).toBe(true);
    expect(matchesKindFilter("Media", "html")).toBe(true);
    expect(matchesKindFilter("Media", "md")).toBe(false);
    expect(matchesKindFilter("Docs", "md")).toBe(true);
    expect(matchesKindFilter("Plans", "plan")).toBe(true);
  });

  it("admits nothing for a filter it does not know", () => {
    expect(matchesKindFilter("Sketches", "md")).toBe(false);
  });
});

describe("FileBadge", () => {
  it("pairs each size with its own font size and keeps leading-none", () => {
    render(<FileBadge kind="md" size={32} />);
    const classes = screen.getByText("MD").className;
    expect(classes).toContain("h-[32px]");
    expect(classes).toContain("text-[9.5px]");
    expect(classes).toContain("leading-none");
    expect(classes).toContain("rounded-lg");
  });

  it("derives the code badge text from the language", () => {
    render(<FileBadge kind="code" language="rs" />);
    expect(screen.getByText("RS")).toBeInTheDocument();
  });
});

describe("Tab", () => {
  it("exposes selection to assistive tech, which the design does not", () => {
    render(
      <>
        <Tab label="Preview" active />
        <Tab label="Diff" active={false} />
      </>,
    );
    expect(screen.getByRole("tab", { name: "Preview" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByRole("tab", { name: "Diff" })).toHaveAttribute(
      "aria-selected",
      "false",
    );
  });

  it("keeps the size's line-height and the flush -1px underline", () => {
    render(<Tab label="Preview" active size="library" />);
    const classes = screen.getByRole("tab").className;
    expect(classes).toContain("text-base-plus");
    expect(classes).toContain("leading-[normal]");
    expect(classes).toContain("-mb-px");
    expect(classes).toContain("border-b-ink");
  });
});
