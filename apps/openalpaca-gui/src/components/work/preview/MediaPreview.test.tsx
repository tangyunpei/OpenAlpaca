import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ImagePreview } from "./MediaPreview";

const SRC = "https://127.0.0.1:4173/v1/artifacts/art-1/content?token=t";

describe("ImagePreview (§3.25f) — load failure", () => {
  it("renders the bytes when the src loads", () => {
    render(<ImagePreview filename="shot.png" size="compact" src={SRC} />);
    expect(screen.getByRole("img", { name: "shot.png" })).toBeInTheDocument();
  });

  it("swaps to the dashed placeholder and a sentence when the image fails to load", () => {
    render(<ImagePreview filename="shot.png" size="compact" src={SRC} />);
    const img = screen.getByRole("img", { name: "shot.png" });

    fireEvent.error(img);

    // The broken `<img>` is gone, not left behind under the placeholder.
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    expect(screen.getByText("shot.png")).toBeInTheDocument();
    expect(
      screen.getByText(
        "The image could not be loaded — Export or Reveal opens it in its own app.",
      ),
    ).toBeInTheDocument();
  });

  it("does not claim a load failure when there was never a src", () => {
    render(
      <ImagePreview
        filename="shot.png"
        size="compact"
        src={null}
        note="This file is no longer on disk. The daemon kept its record, so the history below is still real."
      />,
    );
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
    expect(
      screen.getByText(
        "This file is no longer on disk. The daemon kept its record, so the history below is still real.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/could not be loaded/)).not.toBeInTheDocument();
  });

  it("gives a fresh src its own chance after a previous src failed", () => {
    const { rerender } = render(
      <ImagePreview filename="shot.png" size="compact" src={SRC} />,
    );
    fireEvent.error(screen.getByRole("img", { name: "shot.png" }));
    expect(screen.queryByRole("img")).not.toBeInTheDocument();

    rerender(
      <ImagePreview
        filename="shot.png"
        size="compact"
        src="https://127.0.0.1:4173/v1/artifacts/art-1/content?token=fresh"
      />,
    );
    expect(screen.getByRole("img", { name: "shot.png" })).toBeInTheDocument();
  });
});
