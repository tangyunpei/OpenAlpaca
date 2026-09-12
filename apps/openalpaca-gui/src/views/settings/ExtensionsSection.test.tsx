import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  ExtensionRow,
  ExtensionVerb,
  ManifestSummary,
} from "@/lib/api/types";
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
  installed: [] as string[],
  validated: [] as string[],
  updated: [] as Array<{ id: string; path: string }>,
  declared: [] as unknown[],
  uninstalled: [] as Array<{ kind: string; id: string; keepData: boolean }>,
  fail: null as string | null,
  /** Every `POST …/config` the Configure form sent. */
  configWrites: [] as Array<{ id: string; key: string; value: string }>,
  /** The daemon's refusal sentence for the next config write, if any. */
  configFail: null as string | null,
}));

/** A mutation double that records its input and calls the caller back. */
function recorder<I, R>(record: (input: I) => void, reply: (input: I) => R) {
  return () => ({
    isPending: false,
    mutate: (
      input: I,
      options?: {
        onSuccess?: (result: R) => void;
        onError?: (error: Error) => void;
      },
    ) => {
      record(input);
      if (state.fail !== null) options?.onError?.(new Error(state.fail));
      else options?.onSuccess?.(reply(input));
    },
  });
}

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
  useSetExtensionConfig: () => ({
    isPending: false,
    mutate: (
      input: { id: string; key: string; value: string },
      options?: {
        onSuccess?: () => void;
        onError?: (error: Error) => void;
      },
    ) => {
      state.configWrites.push(input);
      if (state.configFail !== null)
        options?.onError?.(new Error(state.configFail));
      else options?.onSuccess?.();
    },
  }),
  useInstallPlugin: recorder<string, { extension: ExtensionRow }>(
    (path) => state.installed.push(path),
    (path) => ({ extension: extensionRow({ id: basename(path) }) }),
  ),
  useValidatePlugin: recorder<
    string,
    { manifest: ManifestSummary; installed: boolean }
  >(
    (path) => state.validated.push(path),
    (path) => ({ manifest: manifestSummary(basename(path)), installed: false }),
  ),
  useUpdatePlugin: recorder<
    { id: string; path: string },
    {
      extension: ExtensionRow;
      consent_reset: boolean;
      added_capabilities: string[];
    }
  >(
    (input) => state.updated.push(input),
    (input) => ({
      extension: extensionRow({ id: input.id, version: "9.9.9" }),
      consent_reset: false,
      added_capabilities: [],
    }),
  ),
  useAddMcpServer: recorder<unknown, { extension: ExtensionRow }>(
    (declaration) => state.declared.push(declaration),
    () => ({ extension: extensionRow({ kind: "mcp", id: "github" }) }),
  ),
  useUninstallExtension: recorder<
    { kind: string; id: string; keepData: boolean },
    { removed: string; trashed: string | null }
  >(
    (input) => state.uninstalled.push(input),
    (input) => ({
      removed: input.id,
      trashed: `/plugins/.trash/${input.id}-2026`,
    }),
  ),
}));

const basename = (path: string) =>
  path.split("/").filter(Boolean).pop() ?? path;

function manifestSummary(name: string): ManifestSummary {
  return {
    name,
    version: "1.4.0",
    description: "A stub plugin",
    entry: "./run.sh",
    capabilities: ["notes_write"],
    virtual_capabilities: [],
    types: { tool: true, skill: false },
    required_config_keys: ["token"],
    sensitive_config_keys: ["token"],
  };
}

beforeEach(() => {
  state.rows = [];
  state.calls = [];
  state.removed = [];
  state.installed = [];
  state.validated = [];
  state.updated = [];
  state.declared = [];
  state.uninstalled = [];
  state.fail = null;
  state.configWrites = [];
  state.configFail = null;
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

  // ── GAP-24 ───────────────────────────────────────────────────────

  /**
   * The dry run puts the manifest on screen **before** anything is copied, and
   * the install then says the one thing that is left: nothing is running until
   * the plugin is approved.
   */
  it("previews a plugin before copying it, and says the install started nothing", async () => {
    render(<ExtensionsSection />);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Add extension" }));

    await user.type(
      screen.getByRole("textbox", { name: "Plugin directory" }),
      "/src/notion",
    );
    await user.click(screen.getByRole("button", { name: "Check" }));

    expect(state.validated).toEqual(["/src/notion"]);
    expect(state.installed).toEqual([]);
    expect(screen.getByText("notion v1.4.0")).toBeInTheDocument();
    expect(screen.getByText("Asks for: notes_write")).toBeInTheDocument();
    expect(screen.getByText("Needs configuring: token")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Install" }));
    expect(state.installed).toEqual(["/src/notion"]);
    expect(useUiStore.getState().toast).toBe(
      "notion installed — approve it to start it",
    );
  });

  it("declares an MCP server from the same add panel", async () => {
    render(<ExtensionsSection />);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Add extension" }));
    await user.click(screen.getByRole("radio", { name: "MCP server" }));

    await user.type(
      screen.getByRole("textbox", { name: "Server name" }),
      "github",
    );
    await user.type(screen.getByRole("textbox", { name: "Command" }), "npx");
    await user.type(
      screen.getByRole("textbox", { name: "Arguments" }),
      "-y @modelcontextprotocol/server-github",
    );
    await user.click(screen.getByRole("button", { name: "Add server" }));

    expect(state.declared).toEqual([
      {
        name: "github",
        transport: "stdio",
        enabled: true,
        command: "npx",
        args: ["-y", "@modelcontextprotocol/server-github"],
      },
    ]);
  });

  /** A refusal word becomes copy here too, not a raw `invalid_manifest`. */
  it("turns an install refusal into copy the owner can act on", async () => {
    state.fail = "invalid_manifest";
    render(<ExtensionsSection />);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Add extension" }));
    await user.type(
      screen.getByRole("textbox", { name: "Plugin directory" }),
      "/src/broken",
    );
    await user.click(screen.getByRole("button", { name: "Install" }));

    expect(
      screen.getByText(
        "That directory's plugin.toml cannot make a plugin, so nothing was copied.",
      ),
    ).toBeInTheDocument();
  });

  it("updates a plugin from its own row", async () => {
    state.rows = [extensionRow({ id: "notion", state: "enabled" })];
    render(<ExtensionsSection />);
    const user = userEvent.setup();

    await user.click(
      screen.getByRole("button", { name: "More actions for notion" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "Update…" }));
    await user.type(
      screen.getByRole("textbox", { name: "Replacement directory for notion" }),
      "/src/notion-2",
    );
    await user.click(screen.getByRole("button", { name: "Update" }));

    expect(state.updated).toEqual([{ id: "notion", path: "/src/notion-2" }]);
    expect(useUiStore.getState().toast).toBe("notion updated to v9.9.9");
  });

  /**
   * Two deliberate clicks, and the confirmation says what is actually true:
   * the directory is moved, not deleted — and the toast names where it went.
   */
  it("confirms an uninstall and reports where the directory went", async () => {
    state.rows = [extensionRow({ id: "notion", state: "enabled" })];
    render(<ExtensionsSection />);
    const user = userEvent.setup();

    await user.click(
      screen.getByRole("button", { name: "More actions for notion" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "Uninstall…" }));
    expect(state.uninstalled).toEqual([]);
    expect(
      screen.getByText(
        "The directory is moved to plugins/.trash/, not deleted.",
      ),
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "Confirm uninstalling notion" }),
    );
    expect(state.uninstalled).toEqual([
      { kind: "plugin", id: "notion", keepData: true },
    ]);
    expect(useUiStore.getState().toast).toBe(
      "notion uninstalled — kept at /plugins/.trash/notion-2026",
    );
  });

  /**
   * Removing a server's declaration requires it to be `Disabled` first, so the
   * item is not offered on a live one — and `Update` is plugins only, because
   * an MCP declaration is a block in the owner's own file.
   */
  it("offers Uninstall on an MCP server only once it is turned off", async () => {
    state.rows = [
      extensionRow({ kind: "mcp", id: "live", state: "enabled" }),
      extensionRow({
        kind: "mcp",
        id: "off",
        state: "disabled",
        enabled: false,
      }),
    ];
    render(<ExtensionsSection />);
    const user = userEvent.setup();

    await user.click(
      screen.getByRole("button", { name: "More actions for live" }),
    );
    expect(screen.queryByRole("menuitem", { name: "Uninstall…" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Update…" })).toBeNull();

    await user.click(screen.getByRole("button", { name: /1 turned off/ }));
    await user.click(
      screen.getByRole("button", { name: "More actions for off" }),
    );
    expect(
      screen.getByRole("menuitem", { name: "Uninstall…" }),
    ).toBeInTheDocument();
  });
  /**
   * The Configure form offers a field for every missing key, including one the
   * manifest declares `sensitive` — the row carries no sensitivity flag, so it
   * cannot tell them apart — and the daemon always refuses that one with a
   * `400` whose sentence names the key. Left verbatim it is a dead end; the row
   * has to say what to do instead, and nothing must have been written.
   */
  it("renders the sensitive-key refusal as the hand-edit path it needs", async () => {
    state.rows = [
      extensionRow({
        kind: "plugin",
        id: "vault",
        state: "failed",
        reason: "needs_config",
        actionable: true,
        missing_config_keys: ["api_key"],
      }),
    ];
    state.configFail =
      "permission denied: config key 'api_key' of plugin 'vault' is declared sensitive; " +
      "store it as a secret reference, not in the plugin's TOML";
    render(<ExtensionsSection />);
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "Configure" }));
    await user.type(screen.getByLabelText("api_key"), "sk-live-1");
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(state.configWrites).toEqual([
      { id: "vault", key: "api_key", value: "sk-live-1" },
    ]);
    expect(
      screen.getByText(/api_key is a secret this plugin declares/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /~\/\.openalpaca\/plugins\/\.config\/vault\.toml by hand/,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/declared sensitive;/)).toBeNull();
  });
});
