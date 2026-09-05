/**
 * The §9.2 row table, one case per row.
 *
 * These assert the *contract*, not the copy: which control is drawn, which
 * word and tone the tag carries, which affordance is offered, and where the
 * row says the bit lives.
 */

import { describe, expect, it } from "vitest";

import { extensionRow } from "./extension-fixture";
import {
  extensionErrorCopy,
  extensionRowView,
  orderExtensions,
} from "./extension-row";

describe("extensionRowView (ADR-030 §9.2)", () => {
  it("enabled — a live switch, the `active` word, and what it serves", () => {
    const view = extensionRowView(
      extensionRow({
        kind: "mcp",
        id: "github",
        state: "enabled",
        transport: "stdio",
        tools: ["github__create_issue", "github__list_prs"],
      }),
    );

    expect(view).toMatchObject({
      tag: "active",
      tone: "live",
      control: "toggle",
      toggleChecked: true,
      toggleDisabled: false,
      rank: 1,
    });
    expect(view.description).toBe("2 tools · stdio");
    // Applying an edited declaration is not a primary control (§9.2).
    expect(view.menu).toEqual(["reload"]);
    expect(view.actions).toEqual([]);
  });

  it("disabled — the switch is off and the row names where the bit lives", () => {
    const mcp = extensionRowView(
      extensionRow({
        kind: "mcp",
        id: "github",
        state: "disabled",
        enabled: false,
      }),
    );
    expect(mcp).toMatchObject({
      tag: "disabled",
      tone: "neutral",
      toggleChecked: false,
      description: "Turned off",
      rank: 2,
    });
    expect(mcp.secondary).toBe(
      "config/mcp.toml → [servers.github] enabled = false",
    );

    const plugin = extensionRowView(
      extensionRow({ id: "notion", state: "disabled", enabled: false }),
    );
    expect(plugin.secondary).toBe("plugins/.permissions.toml");
  });

  it("enabling / disabling — inert with the reason, reported literally", () => {
    expect(extensionRowView(extensionRow({ state: "enabling" }))).toMatchObject(
      {
        tag: "loading",
        tone: "neutral",
        toggleDisabled: true,
        disabledReason: "connecting…",
      },
    );
    expect(
      extensionRowView(extensionRow({ state: "disabling" })),
    ).toMatchObject({ tag: "loading", disabledReason: "shutting down…" });
  });

  it("unapproved/never_seen — no switch, the declared list, and the bit's direction", () => {
    const view = extensionRowView(
      extensionRow({
        id: "risky",
        state: "unapproved",
        reason: "never_seen",
        consent: "pending",
        enabled: true,
        // Static, from the manifest — never runtime `tools`, which is empty
        // here (X-19).
        declared: {
          capabilities: ["fs_write", "net_connect"],
          virtual_capabilities: [],
          types: { tool: true },
        },
      }),
    );

    expect(view).toMatchObject({
      tag: "waiting-approval",
      tone: "asks",
      control: "none",
      rank: 0,
    });
    expect(view.actions).toEqual(["approve", "deny"]);
    expect(view.description).toBe(
      "Asks for: fs_write, net_connect — starts on approval",
    );
    expect(view.secondary).toBe("plugins/.permissions.toml");
  });

  it("unapproved/never_seen — the suffix follows the bit, which is real either way", () => {
    expect(
      extensionRowView(
        extensionRow({
          state: "unapproved",
          reason: "never_seen",
          enabled: false,
        }),
      ).description,
    ).toBe("Declares no capabilities — stays off after approval");
  });

  it("unapproved/capabilities_grew — the delta, not the whole manifest list", () => {
    const view = extensionRowView(
      extensionRow({
        state: "unapproved",
        reason: "capabilities_grew",
        added_capabilities: ["fs_write", "net_connect"],
        declared: {
          capabilities: ["fs_read", "fs_write", "net_connect"],
          virtual_capabilities: [],
          types: { tool: true },
        },
      }),
    );

    expect(view.description).toBe(
      "Now also asks for: fs_write, net_connect — starts on approval",
    );
    expect(view.actions).toEqual(["approve", "deny"]);
  });

  it("unapproved/denied — Approve only, and the tone is not an alarm", () => {
    const view = extensionRowView(
      extensionRow({
        state: "unapproved",
        reason: "denied",
        consent: "denied",
        enabled: true,
      }),
    );

    expect(view).toMatchObject({
      tag: "denied",
      tone: "neutral",
      control: "none",
    });
    expect(view.actions).toEqual(["approve"]);
    expect(view.description).toBe(
      "You denied this plugin — starts on approval",
    );
  });

  it("failed/needs_authorization — asks, the hint, and the switch stays ON", () => {
    const view = extensionRowView(
      extensionRow({
        kind: "mcp",
        state: "failed",
        reason: "needs_authorization",
        actionable: true,
        detail: "HTTP 401 from https://api.example.test",
        hint: "https://example.test/authorize",
      }),
    );

    expect(view).toMatchObject({
      tag: "needs-auth",
      tone: "asks",
      control: "toggle",
      toggleChecked: true,
      rank: 0,
    });
    expect(view.authorizeUrl).toBe("https://example.test/authorize");
  });

  it("failed/needs_config — the missing keys, and Configure only where the route exists", () => {
    const plugin = extensionRowView(
      extensionRow({
        id: "notion",
        state: "failed",
        reason: "needs_config",
        actionable: true,
        missing_config_keys: ["api_key", "workspace"],
      }),
    );
    expect(plugin).toMatchObject({ tag: "needs-config", tone: "asks" });
    expect(plugin.description).toBe("Missing: api_key, workspace");
    expect(plugin.actions).toEqual(["configure"]);
    expect(plugin.secondary).toBe("plugins/.config/notion.toml");

    // `POST …/config` is plugins-only — `kind=mcp` is 409
    // `unsupported_for_kind`, so the row names the file instead of offering a
    // button that could only be refused.
    const mcp = extensionRowView(
      extensionRow({
        kind: "mcp",
        id: "github",
        state: "failed",
        reason: "needs_config",
        actionable: true,
        missing_config_keys: ["token"],
      }),
    );
    expect(mcp.actions).toEqual([]);
    expect(mcp.secondary).toBe("config/mcp.toml → [servers.github]");
  });

  it("failed/config_invalid — the parse error, quoted and not interpreted", () => {
    const view = extensionRowView(
      extensionRow({
        id: "broken",
        state: "failed",
        reason: "config_invalid",
        actionable: true,
        detail: "expected `=` at line 4",
        enabled: true,
      }),
    );

    expect(view).toMatchObject({
      tag: "config-invalid",
      tone: "asks",
      toggleChecked: true,
    });
    expect(view.description).toBe('"expected `=` at line 4"');
  });

  it("failed/config_invalid — the toggle follows the bit, including the X-3 exception", () => {
    // A plugin parked at scan with a pre-set `enabled = false` entry: the
    // switch is OFF because the bit is, not because the row failed.
    expect(
      extensionRowView(
        extensionRow({
          state: "failed",
          reason: "config_invalid",
          actionable: true,
          enabled: false,
        }),
      ).toggleChecked,
    ).toBe(false);
  });

  it("failed/crashed — warn, Retry, and the switch stays ON", () => {
    const view = extensionRowView(
      extensionRow({
        kind: "mcp",
        id: "github",
        state: "failed",
        reason: "crashed",
        actionable: false,
        enabled: true,
        detail: "broken pipe",
        since: "2026-09-01T10:04:00Z",
      }),
    );

    // "enabled but not working" is answered directly: the owner asked for it
    // to be on, so the switch says on and Retry means something.
    expect(view).toMatchObject({
      tag: "crashed",
      tone: "warn",
      control: "toggle",
      toggleChecked: true,
      rank: 0,
    });
    expect(view.actions).toEqual(["retry"]);
    expect(view.description).toContain('"broken pipe"');
    expect(view.description).toContain("since 1 Sep");
  });

  it("failed/unreachable — the same not-actionable rendering as a crash", () => {
    expect(
      extensionRowView(
        extensionRow({
          state: "failed",
          reason: "unreachable",
          actionable: false,
          detail: "connection refused",
        }),
      ),
    ).toMatchObject({ tag: "crashed", tone: "warn", actions: ["retry"] });
  });

  it("orphaned — inert switch, Remove, and where the declaration should be", () => {
    const view = extensionRowView(
      extensionRow({ id: "ghost", state: "orphaned", enabled: true }),
    );

    expect(view).toMatchObject({
      tag: "orphaned",
      tone: "warn",
      toggleDisabled: true,
      rank: 0,
    });
    expect(view.actions).toEqual(["remove"]);
    expect(view.description).toBe(
      "declaration not found at plugins/ghost/plugin.toml",
    );
  });

  // §4/§8: `enabled` is `null` on the two rows whose bit nobody can read, and
  // every verb over them is `409 store_unreadable`.
  it("an unreadable bit is not `false` — the switch is inert and says why", () => {
    const view = extensionRowView(
      extensionRow({ kind: "mcp", id: "config/mcp.toml", enabled: null }),
    );

    expect(view.toggleChecked).toBe(false);
    expect(view.toggleDisabled).toBe(true);
    expect(view.disabledReason).toMatch(/cannot read/);
  });
});

describe("orderExtensions (G-4)", () => {
  it("puts degraded rows first and disabled ones last, order preserved within", () => {
    const rows = [
      extensionRow({ id: "a-disabled", state: "disabled", enabled: false }),
      extensionRow({ id: "b-enabled", state: "enabled" }),
      extensionRow({ id: "c-failed", state: "failed", reason: "crashed" }),
      extensionRow({ id: "d-enabled", state: "enabled" }),
      extensionRow({
        id: "e-unapproved",
        state: "unapproved",
        reason: "never_seen",
      }),
    ];

    expect(orderExtensions(rows).map((entry) => entry.row.id)).toEqual([
      "c-failed",
      "e-unapproved",
      "b-enabled",
      "d-enabled",
      "a-disabled",
    ]);
  });
});

describe("extensionErrorCopy (§8's flat envelope)", () => {
  it("turns each refusal word into something a person can act on", () => {
    expect(extensionErrorCopy("not_loaded")).toMatch(/Nothing is loaded/);
    expect(extensionErrorCopy("store_unreadable")).toMatch(/cannot read/);
    expect(extensionErrorCopy("not_orphaned")).toMatch(/still declared/);
    expect(extensionErrorCopy("orphaned")).toMatch(/only Remove/);
    expect(extensionErrorCopy("unsupported_for_kind")).toMatch(
      /does not apply/,
    );
  });

  it("passes anything it does not recognise through verbatim", () => {
    expect(extensionErrorCopy("Request failed with status 500")).toBe(
      "Request failed with status 500",
    );
  });
});
