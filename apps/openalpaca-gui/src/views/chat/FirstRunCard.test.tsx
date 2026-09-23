/** The first-run card on its own: what it says, and its one way out (D-G). */

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { FIRST_RUN_ACTION, FIRST_RUN_HINT, FIRST_RUN_TITLE } from "./first-run";
import { FirstRunCard } from "./FirstRunCard";

describe("the first-run card", () => {
  it("says what is wrong in the daemon's words and offers one way out", async () => {
    const onOpenSettings = vi.fn();
    render(
      <FirstRunCard
        llm={{
          default_model: null,
          default_model_routable: false,
          effective_default_model: null,
        }}
        onOpenSettings={onOpenSettings}
      />,
    );

    const card = screen.getByRole("status", { name: "Set up a model" });
    expect(card).toHaveTextContent(FIRST_RUN_TITLE);
    expect(card).toHaveTextContent(
      "No chat model is configured, and none is available. Open Settings → Models & keys and turn a provider on; a local one needs no key.",
    );
    expect(card).toHaveTextContent(FIRST_RUN_HINT);

    await userEvent.click(
      screen.getByRole("button", { name: FIRST_RUN_ACTION }),
    );
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
  });
});
