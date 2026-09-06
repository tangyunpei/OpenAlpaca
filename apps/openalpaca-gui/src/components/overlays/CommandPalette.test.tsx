import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render as renderRaw, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { useUiStore } from "@/stores/ui";

import { CommandPalette } from "./CommandPalette";

const tasks = vi.hoisted(() => ({
  data: [{ id: "b41c8e02", title: "Connector audit" }],
}));

vi.mock("@/hooks/useTasks", () => ({
  useTasks: () => tasks,
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

/** The rows `GET /v1/artifacts?q=` answers with; empty unless a test sets it. */
let hits: { id: string; name: string }[] = [];
let searches: string[] = [];

function render(ui: React.ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  const wrap = (node: React.ReactElement) => (
    <QueryClientProvider client={client}>{node}</QueryClientProvider>
  );
  const result = renderRaw(wrap(ui));
  return {
    ...result,
    rerender: (next: React.ReactElement) => result.rerender(wrap(next)),
  };
}

beforeEach(() => {
  hits = [];
  searches = [];
  resetConnection();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown) => {
      const url = new URL(String(input));
      searches.push(url.searchParams.get("q") ?? "");
      return new Response(
        JSON.stringify({
          artifacts: hits.map((hit) => ({
            ...hit,
            kind: "markdown",
            mime_type: "text/markdown",
            size_bytes: 1,
            task_id: null,
            task_title: null,
            agent_id: null,
            agent_template_id: null,
            version: 1,
            version_count: 1,
            summary: null,
            metadata: null,
            created_at: "2026-09-05T10:00:00Z",
            updated_at: "2026-09-05T10:00:00Z",
            origin: "produced",
            pinned: false,
            missing: false,
            path: `/tmp/${hit.name}`,
            project_root: null,
            rel_path: hit.name,
          })),
          total: hits.length,
        }),
        { status: 200 },
      );
    }),
  );
  useUiStore.setState({
    paletteOpen: true,
    view: "chat",
    dense: false,
    settingsSectionId: "connection",
    steerTargetRunId: null,
    openArtifactId: null,
  });
});

describe("CommandPalette (§3.33)", () => {
  it("renders nothing while the palette is closed", () => {
    useUiStore.setState({ paletteOpen: false });
    const { container } = render(<CommandPalette />);
    expect(container).toBeEmptyDOMElement();
  });

  it("focuses the input on mount", () => {
    render(<CommandPalette />);
    expect(screen.getByRole("combobox")).toHaveFocus();
  });

  it("filters as you type over group + label", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);
    await user.type(screen.getByRole("combobox"), "librar");
    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(1);
    expect(options[0]).toHaveTextContent("Library — artifacts");
  });

  it("says so when nothing matches", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);
    await user.type(screen.getByRole("combobox"), "zzzz");
    await waitFor(() =>
      expect(
        screen.getByText("No commands or files match that."),
      ).toBeInTheDocument(),
    );
    expect(screen.queryAllByRole("option")).toHaveLength(0);
  });

  it("moves the selection with the arrow keys and runs it with Enter", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);

    // Row 0 is `New background job`; row 1 is the Steer row for the active run.
    await user.keyboard("{ArrowDown}");
    expect(screen.getAllByRole("option")[1]).toHaveAttribute(
      "aria-selected",
      "true",
    );

    await user.keyboard("{Enter}");
    const state = useUiStore.getState();
    expect(state.paletteOpen).toBe(false);
    expect(state.steerTargetRunId).toBe("b41c8e02");
  });

  it("wraps the selection at both ends", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);
    const count = screen.getAllByRole("option").length;
    await user.keyboard("{ArrowUp}");
    expect(screen.getAllByRole("option")[count - 1]).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("dispatches a Go command into the ui store", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);
    await user.click(screen.getByRole("option", { name: /Work — all runs/ }));
    expect(useUiStore.getState().view).toBe("work");
    expect(useUiStore.getState().paletteOpen).toBe(false);
  });

  it("opens Settings on the section the command names", async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);
    await user.click(
      screen.getByRole("option", { name: /Settings — tools & extensions/ }),
    );
    expect(useUiStore.getState().view).toBe("settings");
    expect(useUiStore.getState().settingsSectionId).toBe("tools");
  });

  it("only offers Approve when the chat lane has a pending confirmation", async () => {
    const onApprove = vi.fn();
    const { rerender } = render(<CommandPalette />);
    expect(screen.queryByRole("option", { name: /Approve/ })).toBeNull();

    rerender(
      <CommandPalette
        pendingConfirmation={{ toolName: "shell_execute", onApprove }}
      />,
    );
    await userEvent
      .setup()
      .click(
        screen.getByRole("option", { name: /Approve pending shell_execute/ }),
      );
    expect(onApprove).toHaveBeenCalledOnce();
  });

  it("offers no Find row, and asks for nothing, until something is typed", () => {
    render(<CommandPalette />);
    expect(screen.queryByRole("option", { name: /\.md/ })).toBeNull();
    expect(searches).toHaveLength(0);
  });

  it("searches the library for what was typed and opens the hit", async () => {
    hits = [{ id: "art-1", name: "connector-audit-findings.md" }];
    const user = userEvent.setup();
    render(<CommandPalette />);

    await user.type(screen.getByRole("combobox"), "findings");

    const row = await screen.findByRole("option", {
      name: /connector-audit-findings\.md/,
    });
    expect(searches.at(-1)).toBe("findings");

    await user.click(row);
    const state = useUiStore.getState();
    expect(state.paletteOpen).toBe(false);
    expect(state.view).toBe("library");
    expect(state.openArtifactId).toBe("art-1");
    expect(state.libraryTab).toBe("preview");
  });
});
