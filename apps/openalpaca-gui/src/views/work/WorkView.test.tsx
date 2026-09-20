/**
 * The view's chrome and its behaviour with no daemon reachable — which is what
 * a test environment is. Nothing here mocks a run into existence: the point is
 * that the empty and unreachable states say what is true.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { useUiStore } from "@/stores/ui";

import WorkView from "./WorkView";

function renderView() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <WorkView />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  useUiStore.setState({ selectedRunId: null });
});

describe("WorkView (§2.3, §5.2)", () => {
  it("renders the header, its counts and the list/detail resizer", () => {
    renderView();
    expect(screen.getByRole("heading", { name: "Work" })).toBeInTheDocument();
    expect(screen.getByText("0 active · 0 done")).toBeInTheDocument();
    expect(
      screen.getByRole("separator", { name: /Resize the run list/ }),
    ).toBeInTheDocument();
  });

  it("prompts for a selection instead of rendering an empty run", () => {
    renderView();
    expect(
      screen.getByText("Select a run to see what it did."),
    ).toBeInTheDocument();
  });

  it("says the daemon is unreachable rather than showing an empty list", async () => {
    renderView();
    expect(
      await screen.findByText("Could not reach the daemon."),
    ).toBeInTheDocument();
  });
});
