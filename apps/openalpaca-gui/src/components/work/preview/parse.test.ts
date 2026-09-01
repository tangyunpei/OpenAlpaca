import { describe, expect, it } from "vitest";

import {
  columnFlex,
  columnValues,
  detectDelimiter,
  isBooleanColumn,
  isMonoColumn,
  parseDelimited,
  parsePlan,
  parseTable,
  parseTerminal,
  planProgress,
} from "./parse";

describe("parseTable", () => {
  it("takes the first row as the header and pads ragged rows", () => {
    const table = parseTable(
      "tool,calls,ok\nshell_execute,41,yes\nfile_edit,9",
    );
    expect(table.columns).toEqual(["tool", "calls", "ok"]);
    expect(table.rows).toEqual([
      ["shell_execute", "41", "yes"],
      ["file_edit", "9", ""],
    ]);
  });

  it("keeps a delimiter inside a quoted field", () => {
    const table = parseTable('name,note\nweb_fetch,"reads, then writes"');
    expect(table.rows[0]).toEqual(["web_fetch", "reads, then writes"]);
  });

  it("unescapes a doubled quote", () => {
    expect(parseDelimited('a\n"say ""hi"""')[1]).toEqual(['say "hi"']);
  });

  it("detects tab-separated data", () => {
    expect(detectDelimiter("a\tb\tc\n1\t2\t3")).toBe("\t");
    expect(parseTable("a\tb\n1\t2").columns).toEqual(["a", "b"]);
  });

  it("has no columns and no rows for empty bytes", () => {
    expect(parseTable("")).toEqual({ columns: [], rows: [] });
  });
});

describe("column classification", () => {
  const table = parseTable(
    "tool,calls,confirms,summary\nshell_execute,41,yes,runs a shell command\nfile_edit,9,no,edits a file",
  );

  it("treats identifier and numeric columns as mono", () => {
    expect(isMonoColumn(columnValues(table, 0))).toBe(true);
    expect(isMonoColumn(columnValues(table, 1))).toBe(true);
  });

  it("leaves prose columns in the sans face", () => {
    expect(isMonoColumn(columnValues(table, 3))).toBe(false);
  });

  it("colours only a column that is entirely yes/no", () => {
    expect(isBooleanColumn(columnValues(table, 2))).toBe(true);
    expect(isBooleanColumn(columnValues(table, 3))).toBe(false);
  });

  it("weights the first column widest, and narrows a compact third of three", () => {
    expect(columnFlex(0, 3, "compact")).toBe(1.6);
    expect(columnFlex(0, 4, "full")).toBe(2);
    expect(columnFlex(2, 3, "compact")).toBe(0.8);
    expect(columnFlex(2, 4, "full")).toBe(1);
  });
});

describe("parsePlan", () => {
  const PLAN = [
    "# Plan",
    "- [x] read the connector code",
    "- [x] list the gaps",
    "- [!] run cargo tree — awaiting approval",
    "- [ ] write the report",
    "not a step",
  ].join("\n");

  it("reads the three step states", () => {
    expect(parsePlan(PLAN).map((step) => step.state)).toEqual([
      "complete",
      "complete",
      "blocked",
      "pending",
    ]);
  });

  it("splits a trailing note off the label", () => {
    const blocked = parsePlan(PLAN)[2];
    expect(blocked?.label).toBe("run cargo tree");
    expect(blocked?.note).toBe("awaiting approval");
  });

  it("defaults a blocked step's note", () => {
    expect(parsePlan("- [!] hold")[0]?.note).toBe("awaiting approval");
  });

  it("counts progress the way the eyebrow states it", () => {
    expect(planProgress(parsePlan(PLAN))).toBe("2 of 4 complete");
  });

  it("accepts numbered checklists", () => {
    expect(parsePlan("1. [x] one\n2) [ ] two")).toHaveLength(2);
  });
});

describe("parseTerminal", () => {
  it("marks the command echo, not its output", () => {
    const lines = parseTerminal("$ cargo tree\nopenalpaca v0.1.0\n");
    expect(lines).toHaveLength(2);
    expect(lines[0]?.prompt).toBe(true);
    expect(lines[1]?.prompt).toBe(false);
  });

  it("returns nothing for empty output", () => {
    expect(parseTerminal("")).toEqual([]);
  });
});
