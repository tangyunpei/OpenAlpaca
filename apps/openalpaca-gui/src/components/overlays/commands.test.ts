import { describe, expect, it, vi } from "vitest";

import {
  buildCommands,
  filterCommands,
  matchesShortcut,
  shortcutLabel,
  type CommandCatalogInput,
} from "./commands";

function input(
  overrides: Partial<CommandCatalogInput> = {},
): CommandCatalogInput {
  return {
    activeRun: null,
    pendingConfirmation: null,
    goChat: vi.fn(),
    goWork: vi.fn(),
    goLibrary: vi.fn(),
    goSettingsSection: vi.fn(),
    steerRun: vi.fn(),
    toggleDense: vi.fn(),
    ...overrides,
  };
}

describe("buildCommands (§3.33)", () => {
  it("omits Steer and Approve when there is nothing to steer or approve", () => {
    const groups = buildCommands(input()).map((command) => command.group);
    expect(groups).not.toContain("Steer");
    expect(groups).not.toContain("Approve");
  });

  it("never ships a Find row — there is no artifact API to search (GAP-04)", () => {
    const labels = buildCommands(
      input({
        activeRun: { id: "b41c8e02", title: "Connector audit" },
        pendingConfirmation: { toolName: "shell_execute", approve: vi.fn() },
      }),
    ).map((command) => command.label);
    expect(labels.some((label) => label.startsWith("Find"))).toBe(false);
  });

  it("targets the active run by id", () => {
    const steerRun = vi.fn();
    const commands = buildCommands(
      input({
        activeRun: { id: "b41c8e02", title: "Connector audit" },
        steerRun,
      }),
    );
    const steer = commands.find((command) => command.group === "Steer");
    expect(steer?.label).toBe("Steer Connector audit");
    steer?.run();
    expect(steerRun).toHaveBeenCalledWith("b41c8e02");
  });

  it("draws Approve's ↵ without claiming the key useGlobalKeys owns", () => {
    const commands = buildCommands(
      input({
        pendingConfirmation: { toolName: "shell_execute", approve: vi.fn() },
      }),
    );
    const approve = commands.find((command) => command.group === "Approve");
    expect(approve?.shortcut).toBeUndefined();
    expect(shortcutLabel(approve as never)).toBe("↵");
  });

  it("labels modifiers the way the design writes them", () => {
    const commands = buildCommands(input());
    const density = commands.find((command) => command.id === "view.density");
    expect(shortcutLabel(density as never)).toBe("⌘⇧D");
  });
});

describe("filterCommands (§3.33)", () => {
  const commands = buildCommands(input());

  it("returns everything for an empty query", () => {
    expect(filterCommands(commands, "   ")).toHaveLength(commands.length);
  });

  it("matches the group as well as the label", () => {
    const byGroup = filterCommands(commands, "view");
    expect(byGroup.map((command) => command.id)).toEqual(["view.density"]);
  });

  it("is case-insensitive over group + label", () => {
    expect(filterCommands(commands, "LIBRARY").map((c) => c.id)).toEqual([
      "go.library",
    ]);
  });
});

describe("matchesShortcut", () => {
  const event = (
    key: string,
    modifiers: {
      metaKey?: boolean;
      ctrlKey?: boolean;
      shiftKey?: boolean;
    } = {},
  ) => ({
    key,
    metaKey: modifiers.metaKey ?? false,
    ctrlKey: modifiers.ctrlKey ?? false,
    shiftKey: modifiers.shiftKey ?? false,
  });

  it("accepts Ctrl where the design writes ⌘", () => {
    expect(
      matchesShortcut(event("2", { ctrlKey: true }), { key: "2", meta: true }),
    ).toBe(true);
  });

  it("does not fire ⌘⇧D on ⌘D", () => {
    expect(
      matchesShortcut(event("d", { metaKey: true }), {
        key: "d",
        meta: true,
        shift: true,
      }),
    ).toBe(false);
  });

  it("does not fire ⌘2 on a bare 2", () => {
    expect(matchesShortcut(event("2"), { key: "2", meta: true })).toBe(false);
  });
});
