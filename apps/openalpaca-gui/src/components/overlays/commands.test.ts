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
    goMemory: vi.fn(),
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

  it("ships no Find row: those are a query, built by the palette itself", () => {
    const labels = buildCommands(
      input({
        activeRun: { id: "b41c8e02", title: "Connector audit" },
        pendingConfirmation: {
          toolName: "shell_execute",
          hidden: 0,
          count: 1,
          approve: vi.fn(),
        },
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
        pendingConfirmation: {
          toolName: "shell_execute",
          hidden: 0,
          count: 1,
          approve: vi.fn(),
        },
      }),
    );
    const approve = commands.find((command) => command.group === "Approve");
    expect(approve?.shortcut).toBeUndefined();
    expect(shortcutLabel(approve as never)).toBe("↵");
  });

  /**
   * The row approves, so it names the tool as the card does. A label is a
   * string, so the escapes are the plain-text `<U+XXXX>` form.
   */
  it("names the pending tool with its hidden characters shown as escapes", () => {
    const commands = buildCommands(
      input({
        pendingConfirmation: {
          toolName: "srv__rm\u{202E}fdp\u{200B}\n",
          hidden: 0,
          count: 1,
          approve: vi.fn(),
        },
      }),
    );
    const approve = commands.find((command) => command.group === "Approve");
    expect(approve?.label).toBe(
      "Approve pending srv__rm<U+202E>fdp<U+200B><U+000A>",
    );
  });

  /**
   * The row approves and shows the name alone: when the request holds hidden
   * characters anywhere, it says how many and that the card shows them.
   */
  it("counts the whole request's hidden characters and points at its card", () => {
    const label = (hidden: number) =>
      buildCommands(
        input({
          pendingConfirmation: {
            toolName: "shell_execute",
            hidden,
            count: 1,
            approve: vi.fn(),
          },
        }),
      ).find((command) => command.group === "Approve")?.label;

    expect(label(2)).toBe(
      "Approve pending shell_execute · its card shows 2 hidden characters",
    );
    expect(label(1)).toBe(
      "Approve pending shell_execute · its card shows 1 hidden character",
    );
    expect(label(0)).toBe("Approve pending shell_execute");
  });

  /** The row answers the oldest of several pending prompts: it says so. */
  it("says the row approves the oldest of several pending prompts", () => {
    const approve = buildCommands(
      input({
        pendingConfirmation: {
          toolName: "shell_execute",
          hidden: 0,
          count: 3,
          approve: vi.fn(),
        },
      }),
    ).find((command) => command.group === "Approve");
    expect(approve?.label).toBe(
      "Approve pending shell_execute · oldest of 3 pending",
    );
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

  it("reaches the fifth view, Memory, from its own Go row on ⌘4", () => {
    const memory = filterCommands(commands, "memory");
    expect(memory.map((command) => command.id)).toEqual(["go.memory"]);
    expect(shortcutLabel(memory[0] as never)).toBe("⌘4");
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
