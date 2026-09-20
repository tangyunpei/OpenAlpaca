/**
 * `DocumentPreview` — P5, the markdown table in a narrow pane.
 *
 * `marked` emits a `<table>`; the prose class lists had no rule for one, so
 * the browser drew it with no padding and no rules at all and a three-column
 * table's cells read as one word in the Work pane at 1080 points
 * ("Fiber appearancehalo"). The styling is descendant variants on the one
 * sanitized tree, so these assert the rules that reach the elements.
 */

import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { DocumentPreview } from "./DocumentPreview";
import type { PreviewSize } from "./types";

const TABLE = [
  "| Fiber | Appearance | Warmth |",
  "| --- | --- | --- |",
  "| Huacaya | crimped, halo | high |",
  "| Suri | silky, draping | high-end |",
].join("\n");

function prose(size: PreviewSize): HTMLElement {
  const { container } = render(<DocumentPreview source={TABLE} size={size} />);
  const table = container.querySelector("table");
  if (table === null) throw new Error("marked emitted no table");
  const styled = table.parentElement;
  if (styled === null) throw new Error("the table has no prose container");
  return styled;
}

describe.each<PreviewSize>(["compact", "full"])(
  "DocumentPreview — a markdown table (%s, P5)",
  (size) => {
    it("gives the table its own horizontal scroll rather than squeezing it", () => {
      const classes = prose(size).className;
      // max-content inside a capped box: the table takes the width it needs
      // and scrolls inside the pane, the bargain the code blocks already make.
      expect(classes).toContain("[&_table]:block");
      expect(classes).toContain("[&_table]:w-max");
      expect(classes).toContain("[&_table]:max-w-full");
      expect(classes).toContain("[&_table]:overflow-x-auto");
    });

    it("separates the cells — padding and a rule on every row", () => {
      const classes = prose(size).className;
      expect(classes).toMatch(/\[&_th\]:px-\[\d+px\]/);
      expect(classes).toMatch(/\[&_th\]:py-\[\d+px\]/);
      expect(classes).toMatch(/\[&_td\]:px-\[\d+px\]/);
      expect(classes).toMatch(/\[&_td\]:py-\[\d+px\]/);
      expect(classes).toContain("[&_th]:border-b");
      expect(classes).toContain("[&_td]:border-b");
    });

    it("still renders every cell the markdown named", () => {
      const { container } = render(
        <DocumentPreview source={TABLE} size={size} />,
      );
      expect(container.querySelectorAll("th")).toHaveLength(3);
      expect(container.querySelectorAll("tbody tr")).toHaveLength(2);
      expect(container.querySelector("tbody td")?.textContent).toBe("Huacaya");
    });
  },
);
