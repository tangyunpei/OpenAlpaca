import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ArtifactPreview, looksLikePatch } from "./ArtifactPreview";
import type { PreviewMeta } from "./types";

const meta = (patch: Partial<PreviewMeta> = {}): PreviewMeta => ({
  name: "findings.md",
  kind: "md",
  ...patch,
});

describe("ArtifactPreview dispatch (§3.25)", () => {
  it("renders a document from markdown", () => {
    render(
      <ArtifactPreview
        meta={meta({ byline: "v2 of 2 · review_agent" })}
        content={"## Findings\n\nTwo connectors are unwired."}
        size="compact"
      />,
    );
    expect(screen.getByText("Findings")).toBeInTheDocument();
    expect(screen.getByText("Two connectors are unwired.")).toBeInTheDocument();
    expect(screen.getByText("v2 of 2 · review_agent")).toBeInTheDocument();
  });

  it("strips scripts out of agent-authored markdown", () => {
    const { container } = render(
      <ArtifactPreview
        meta={meta()}
        content={"<script>window.stolen = 1</script>\n\nplain text"}
        size="compact"
      />,
    );
    expect(container.querySelector("script")).toBeNull();
  });

  it("renders a table from CSV, mono in the identifier column", () => {
    render(
      <ArtifactPreview
        meta={meta({ name: "audit.csv", kind: "table" })}
        content={"tool,calls,ok\nshell_execute,41,yes"}
        size="compact"
      />,
    );
    expect(
      screen.getByRole("columnheader", { name: "tool" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("cell", { name: "shell_execute" }).className,
    ).toContain("font-mono");
    expect(screen.getByRole("cell", { name: "yes" }).className).toContain(
      "text-green",
    );
  });

  it("renders a plan with its progress eyebrow", () => {
    render(
      <ArtifactPreview
        meta={meta({ name: "plan.md", kind: "plan" })}
        content={"- [x] one\n- [ ] two"}
        size="compact"
      />,
    );
    expect(screen.getByText("1 of 2 complete")).toBeInTheDocument();
  });

  it("renders terminal output and omits an exit code it does not have", () => {
    render(
      <ArtifactPreview
        meta={meta({ name: "cargo-tree.out", kind: "term" })}
        content={"$ cargo tree\nopenalpaca v0.1.0"}
        size="compact"
      />,
    );
    expect(screen.getByText("$ cargo tree")).toBeInTheDocument();
    expect(screen.getByText("cargo-tree.out")).toBeInTheDocument();
    expect(screen.queryByText(/exit/)).not.toBeInTheDocument();
  });

  it("shows an exit code when one is known", () => {
    render(
      <ArtifactPreview
        meta={meta({
          name: "out",
          kind: "term",
          exitCode: 1,
          duration: "1.4s",
        })}
        content={"boom"}
        size="compact"
      />,
    );
    expect(screen.getByText("exit 1 · 1.4s")).toBeInTheDocument();
  });

  it("renders code with a gutter in the full size only", () => {
    const full = render(
      <ArtifactPreview
        meta={meta({
          name: "lib.rs",
          kind: "code",
          addedLines: 2,
          removedLines: 1,
        })}
        content={"fn main() {}\n"}
        size="full"
      />,
    );
    expect(full.getByText("lib.rs")).toBeInTheDocument();
    expect(full.getByText("+2")).toBeInTheDocument();
    expect(full.container.querySelector(".w-\\[44px\\]")).not.toBeNull();
    full.unmount();

    const compact = render(
      <ArtifactPreview
        meta={meta({ name: "lib.rs", kind: "code" })}
        content={"fn main() {}\n"}
        size="compact"
      />,
    );
    expect(compact.container.querySelector(".w-\\[44px\\]")).toBeNull();
  });

  it("keeps the dashed image placeholder when the bytes cannot be loaded", () => {
    render(
      <ArtifactPreview
        meta={meta({
          name: "shot.png",
          kind: "image",
          width: 1440,
          height: 900,
        })}
        content={null}
        size="compact"
        note="Artifact API not yet available"
      />,
    );
    expect(screen.getByText("shot.png")).toBeInTheDocument();
    expect(screen.getByText("1440 × 900")).toBeInTheDocument();
    expect(
      screen.getByText("Artifact API not yet available"),
    ).toBeInTheDocument();
  });

  it("states the absence rather than rendering an empty document", () => {
    render(
      <ArtifactPreview
        meta={meta()}
        content={null}
        size="compact"
        note="Artifact API not yet available"
      />,
    );
    expect(screen.getByText("Nothing to preview yet.")).toBeInTheDocument();
    expect(
      screen.getByText("Artifact API not yet available"),
    ).toBeInTheDocument();
  });
});

describe("looksLikePatch", () => {
  it("recognises a patch by its hunk marker", () => {
    expect(looksLikePatch("@@ -1,2 +1,3 @@\n a")).toBe(true);
    expect(looksLikePatch("diff --git a/x b/x\n")).toBe(true);
  });

  it("treats a plain source file as source", () => {
    expect(looksLikePatch("fn main() {\n    let x = -1;\n}")).toBe(false);
  });
});
