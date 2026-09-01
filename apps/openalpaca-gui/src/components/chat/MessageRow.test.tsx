import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { AssistantMessage, UserMessage, messageGapClass } from "./MessageRow";

describe("messageGapClass (§8.3)", () => {
  it("is the only thing density changes about a row", () => {
    expect(messageGapClass(false)).toBe("mb-[30px]");
    expect(messageGapClass(true)).toBe("mb-[20px]");
  });
});

describe("UserMessage (§3.10)", () => {
  it("renders the speaker label with the time, and no avatar or bubble", () => {
    const { container } = render(
      <UserMessage text="Audit the connectors" time="14:22" />,
    );
    expect(screen.getByText("You · 14:22")).toBeInTheDocument();
    expect(screen.getByText("Audit the connectors")).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
  });

  it("drops the time rather than printing a placeholder", () => {
    render(<UserMessage text="hi" time={null} />);
    expect(screen.getByText("You")).toBeInTheDocument();
  });

  it("shows the steer pill only for a steered message", () => {
    const { rerender } = render(<UserMessage text="hi" time="14:22" />);
    expect(screen.queryByText(/steer →/)).toBeNull();

    rerender(
      <UserMessage
        text="hi"
        time="14:22"
        steer={{ mode: "steer", label: "connector audit" }}
      />,
    );
    expect(screen.getByText("steer → connector audit")).toBeInTheDocument();

    rerender(
      <UserMessage
        text="hi"
        time="14:22"
        steer={{ mode: "queue", label: "connector audit" }}
      />,
    );
    expect(screen.getByText("follow-up → connector audit")).toBeInTheDocument();
  });
});

describe("AssistantMessage (§3.10, §3.11)", () => {
  it("renders the meta line straight from the done payload", () => {
    render(
      <AssistantMessage
        text="Done."
        meta={{
          model: "claude-sonnet-4-6",
          durationMs: 3800,
          tokensIn: 1284,
          tokensOut: 612,
        }}
      />,
    );
    expect(screen.getByText("Alpaca")).toBeInTheDocument();
    expect(
      screen.getByText("sonnet-4-6 · 3.8s · 1284/612 tok"),
    ).toBeInTheDocument();
  });

  it("shows the thinking indicator before the first delta and no meta", () => {
    render(<AssistantMessage text="" streamPhase="thinking" />);
    expect(screen.getByText("thinking…")).toBeInTheDocument();
    expect(screen.queryByText(/tok$/)).toBeNull();
  });

  it("swaps the indicator for the metadata line on done", () => {
    const { rerender } = render(
      <AssistantMessage text="par" streamPhase="streaming" />,
    );
    expect(screen.queryByText("thinking…")).toBeNull();

    rerender(
      <AssistantMessage
        text="partial then whole"
        streamPhase={null}
        meta={{ model: "claude-sonnet-4-6", durationMs: 1200 }}
      />,
    );
    expect(screen.getByText("sonnet-4-6 · 1.2s")).toBeInTheDocument();
  });

  it("renders inline code as a mono chip inside the paragraph", () => {
    const { container } = render(
      <AssistantMessage text="run `cargo tree` first" />,
    );
    const code = container.querySelector("code");
    expect(code).not.toBeNull();
    expect(code).toHaveTextContent("cargo tree");
  });
});
