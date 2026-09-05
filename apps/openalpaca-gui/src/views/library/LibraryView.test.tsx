import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";

import { useUiStore } from "@/stores/ui";

import LibraryView from "./LibraryView";

function renderView() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <LibraryView />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  useUiStore.setState({ libraryKind: "All", openArtifactId: null });
});

describe("LibraryView (§2.4, §5.3)", () => {
  it("renders the real chrome — header, every kind chip, the resizer", () => {
    renderView();
    expect(
      screen.getByRole("heading", { name: "Library" }),
    ).toBeInTheDocument();
    for (const label of [
      "All",
      "Docs",
      "Code",
      "Output",
      "Data",
      "Media",
      "Plans",
    ]) {
      expect(screen.getByRole("button", { name: label })).toBeInTheDocument();
    }
    expect(
      screen.getByRole("separator", { name: /Resize library list/ }),
    ).toBeInTheDocument();
  });

  it("omits the file count rather than claiming zero files", () => {
    renderView();
    expect(screen.queryByText(/files/)).toBeNull();
  });

  it("names the missing artifact route instead of inventing rows", () => {
    renderView();
    expect(
      screen.getByText(
        "Nothing in the library yet. Files the agents produce land here.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Artifact API not yet available/),
    ).toHaveTextContent("GET /v1/artifacts?task_id=&kind=&limit=&offset=");
  });

  it("keeps the kind filter live even with no list behind it", async () => {
    const user = userEvent.setup();
    renderView();
    await user.click(screen.getByRole("button", { name: "Code" }));
    expect(useUiStore.getState().libraryKind).toBe("Code");
    expect(screen.getByRole("button", { name: "Code" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  it("invites a selection when nothing is open", () => {
    renderView();
    expect(
      screen.getByText("Select a file to see it here."),
    ).toBeInTheDocument();
  });

  it("explains why a selected artifact will not open", () => {
    useUiStore.setState({ openArtifactId: "findings" });
    renderView();
    expect(
      screen.getByText("This file cannot be opened yet."),
    ).toBeInTheDocument();
  });
});
