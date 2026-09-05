import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Artifact } from "@/lib/api/unbacked";

import { LibraryRow, artifactSubtitle } from "./LibraryRow";

/**
 * A shape-only fixture for the *proposed* resource (GAP-04). It exists to prove
 * the row renders the contract, not to stand in for data anywhere in the app.
 */
const artifact: Artifact = {
  id: "findings",
  name: "connector-audit-findings.md",
  kind: "markdown",
  mime_type: "text/markdown",
  size_bytes: 4096,
  task_id: "b41c8e02",
  task_title: "connector audit",
  agent_id: "agent-1",
  agent_template_id: "review_agent",
  version: 2,
  version_count: 2,
  summary: null,
  metadata: null,
  created_at: "2026-08-31T12:00:00Z",
  updated_at: "2026-08-31T12:00:00Z",
};

describe("LibraryRow (§3.30)", () => {
  it("builds the design's `agent · run · when` subtitle", () => {
    const now = new Date("2026-08-31T12:02:00Z");
    expect(artifactSubtitle(artifact, now)).toBe(
      "review_agent · connector audit · 2m ago",
    );
  });

  it("drops missing attribution rather than filling it in", () => {
    const now = new Date("2026-08-31T12:00:30Z");
    const orphan: Artifact = {
      ...artifact,
      agent_id: null,
      agent_template_id: null,
      task_title: null,
    };
    expect(artifactSubtitle(orphan, now)).toBe("just now");
  });

  it("selects by id and marks only a pinned row", () => {
    const onSelect = vi.fn();
    const { rerender } = render(
      <LibraryRow
        artifact={artifact}
        active={false}
        pinned={false}
        onSelect={onSelect}
      />,
    );
    expect(screen.queryByLabelText("Pinned")).toBeNull();

    screen.getByRole("button").click();
    expect(onSelect).toHaveBeenCalledWith("findings");

    rerender(
      <LibraryRow artifact={artifact} active pinned onSelect={onSelect} />,
    );
    expect(screen.getByLabelText("Pinned")).toBeInTheDocument();
  });
});
