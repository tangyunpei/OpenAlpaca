import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Toast } from "./Toast";

describe("Toast (§3.34)", () => {
  /**
   * The Always-allow toast names a third party's tool, and a name can be one
   * run with no break opportunity in it: it wraps inside the toast's 380px
   * instead of running past the window's edge.
   */
  it("wraps a message too long for the toast instead of running past it", () => {
    render(
      <Toast
        message={`srv__${"x".repeat(200)} added to the allowlist — it won't ask again`}
      />,
    );
    expect(screen.getByText(/added to the allowlist/)).toHaveClass(
      "min-w-0",
      "wrap-anywhere",
    );
  });
});
