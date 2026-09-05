import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ExtensionRow, ExtensionVerb } from "@/lib/api/types";
import { useUiStore } from "@/stores/ui";

import { ExtensionsSection } from "./ExtensionsSection";
import { extensionRow } from "./extension-fixture";

/**
 * The hooks are mocked so the section renders against a known payload; the
 * verb mutation is a double that calls back, because the row-level copy for a
 * refusal is exactly what these tests are checking.
 */
const state = vi.hoisted(() => ({
  rows: [] as unknown[],
  calls: [] as Array<{ id: string; verb: string }>,
  removed: [] as string[],
  fail: null as string | null,
}));

vi.mock("@/hooks/useExtensions", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/hooks/useExtensions")>()),
  useExtensions: () => ({
    data: state.rows,
    isPending: false,
    error: null,
  }),
  useExtensionVerb: () => ({
    isPending: false,
    mutate: (
      input: { id: string; verb: ExtensionVerb },
      options?: {
        onSuccess?: (row: ExtensionRow) => void;
        onError?: (error: Error) => void;
      },
    ) => {
      state.calls.push({ id: input.id, verb: input.verb });
      if (state.fail !== null) options?.onError?.(new Error(state.fail));
      else options?.onSuccess?.(extensionRow({ id: input.id }));
    },
  }),
  useRemoveExtension: () => ({
    isPending: false,
    mutate: (
      id: string,
      options?: { onSuccess?: () => void; onError?: (error: Error) => void },
    ) => {
      state.removed.push(id);
      if (state.fail !== null) options?.onError?.(new Error(state.fail));
      else options?.onSuccess?.();
    },
  }),
  useSetExtensionConfig: () => ({ isPending: false, mutate: vi.fn() }),
}));

beforeEach(() => {
  state.rows = [];
  state.calls = [];
  state.removed = [];
  state.fail = null;
  useUiStore.setState({ toast: null, settingsSectionId: "extensions" });
});

describe("ExtensionsSection (ADR-030 §9.2)", () => {
  it("lists both kinds in one list, each row saying which it is", () => {
    state.rows = [
      extensionRow({ kind: "mcp", id: "github", tools: ["github__x"] }),
      extensionRow({ kind: "plugin", id: "notion", tools: ["notion::x"] }),
    ];
    render(<ExtensionsSection />);

    expect(screen.getByText("github")).toBeInTheDocument();
    expect(screen.getByText("notion")).toBeInTheDocument();
    expect(screen.getByText("MCP")).toBeInTheDocument();
    expect(screen.getByText("Plugin")).toBeInTheDocument();
  });

  /**
   * The live correctness bug this section replaces: the old panel computed
   * `checked={word === "running"}`, so an enabled-but-crashed plugin rendered
   * OFF — and clicking it fired `enable` on something already enabled.
   */
  it("keeps the switch ON for a crashed extension that is still enabled", async () => {
    state.rows = [
      extensionRow({
        kind: "mcp",
        id: "github",
        state: "failed",
        reason: "crashed",
        enabled: true,
        detail: "broken pipe",
      }),
    ];
    render(<ExtensionsSection />);

    const toggle = screen.getByRole("switch", { name: "Enable github" });
    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText("crashed")).toBeInTheDocument();

    // …so the switch turns it off, and Retry is the reload.
    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "Retry" }));
    expect(state.calls).toEqual([{ id: "github", verb: "reload" }]);
  });

  it("offers consent instead of a switch, and never offers Deny twice", () => {
    state.rows = [
      extensionRow({
        id: "risky",
        state: "unapproved",
        reason: "never_seen",
        declared: {
          capabilities: ["fs_write"],
          virtual_capabilities: [],
          types: { tool: true },
        },
      }),
      extensionRow({
        id: "refused",
        state: "unapproved",
        reason: "denied",
        enabled: false,
      }),
    ];
    render(<ExtensionsSection />);

    expect(screen.queryByRole("switch", { name: "Enable risky" })).toBeNull();
    expect(screen.getAllByRole("button", { name: "Approve" })).toHaveLength(2);
    // Only the pending row can be denied; the denied one is already denied.
    expect(screen.getAllByRole("button", { name: "Deny" })).toHaveLength(1);
    expect(
      screen.getByText("Asks for: fs_write — starts on approval"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("You denied this plugin — stays off after approval"),
    ).toBeInTheDocument();
  });

  it("removes an orphan, and offers Remove nowhere else", async () => {
    state.rows = [
      extensionRow({ id: "ghost", state: "orphaned" }),
      extensionRow({ id: "notion", state: "enabled" }),
    ];
    render(<ExtensionsSection />);

    expect(
      screen.getByText("declaration not found at plugins/ghost/plugin.toml"),
    ).toBeInTheDocument();
    const remove = screen.getAllByRole("button", { name: "Remove" });
    expect(remove).toHaveLength(1);

    await userEvent.setup().click(remove[0] as HTMLElement);
    expect(state.removed).toEqual(["ghost"]);
  });

  // §8's flat `{"error": "<word>"}` envelope reaches the client as the word
  // itself; the row is where it has to mean something.
  it("turns a refusal word into row-level copy", async () => {
    state.rows = [extensionRow({ id: "ghost", state: "orphaned" })];
    state.fail = "not_orphaned";
    render(<ExtensionsSection />);

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "Remove" }));

    expect(
      screen.getByText(
        "This extension is still declared, so it cannot be removed.",
      ),
    ).toBeInTheDocument();
    expect(useUiStore.getState().toast).toContain("still declared");
  });

  it("keeps Reload out of the primary controls, in the row's overflow menu", async () => {
    state.rows = [extensionRow({ id: "notion", state: "enabled" })];
    render(<ExtensionsSection />);

    expect(screen.queryByRole("button", { name: "Reload" })).toBeNull();
    const user = userEvent.setup();
    await user.click(
      screen.getByRole("button", { name: "More actions for notion" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "Reload" }));

    expect(state.calls).toEqual([{ id: "notion", verb: "reload" }]);
  });

  it("sorts degraded rows to the top and folds the disabled ones away", async () => {
    state.rows = [
      extensionRow({ id: "off-one", state: "disabled", enabled: false }),
      extensionRow({ id: "live-one", state: "enabled" }),
      extensionRow({ id: "broken-one", state: "failed", reason: "crashed" }),
    ];
    render(<ExtensionsSection />);

    const names = screen.getAllByText(/one$/).map((node) => node.textContent);
    expect(names).toEqual(["broken-one", "live-one"]);
    expect(screen.queryByText("off-one")).toBeNull();

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: /1 turned off/ }));
    expect(screen.getByText("off-one")).toBeInTheDocument();
    expect(screen.getByText("plugins/.permissions.toml")).toBeInTheDocument();
  });

  it("names the install gap rather than pretending an add flow exists", async () => {
    render(<ExtensionsSection />);

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "Add extension" }));
    expect(useUiStore.getState().toast).toMatch(
      /Extension install \/ uninstall not yet available/,
    );
  });
});
