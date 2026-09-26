import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ToolConfirmationBanner } from "./ToolConfirmationBanner";

/**
 * The characters these tests plant, by code point: an override, a zero-width
 * space, a C1 CSI, a carriage return, a tag, a newline and an ideographic
 * space. What a test asserts is that none of them reaches the card as itself.
 */
const PLANTED = [0x202e, 0x200b, 0x9b, 0x0d, 0xe0041, 0x0a, 0x3000];

/** The planted characters `text` still carries raw, as `U+XXXX`. */
function raw(text: string, allowed: readonly number[] = []): string[] {
  const found: string[] = [];
  for (const character of text) {
    const codePoint = character.codePointAt(0) ?? 0;
    if (PLANTED.includes(codePoint) && !allowed.includes(codePoint)) {
      found.push(`U+${codePoint.toString(16).toUpperCase().padStart(4, "0")}`);
    }
  }
  return found;
}

function card(props: Parameters<typeof ToolConfirmationBanner>[0]) {
  render(<ToolConfirmationBanner {...props} />);
  const section = screen.getByRole("alert");
  const eyebrow = section.querySelector("p");
  const box = section.querySelector("pre");
  const escapes = Array.from(
    section.querySelectorAll("[data-hidden-char]"),
    (node) => node.textContent,
  );
  return { section, eyebrow, box, escapes };
}

/**
 * The card is where the person decides, so it shows the exact values that
 * will run: every hidden character as `<U+XXXX>`, set apart from the text,
 * and one line under the arguments saying how many (`lib/hidden-chars`,
 * `docs/GUI_Manual.md` → *Hidden characters on an approval*).
 *
 * Nothing here checks that the tool receives its arguments unchanged: the
 * card borrows them and sends nothing. The answer is `POST
 * /v1/chat/confirmations/{request_id}` with `{approved}` alone, which
 * `ChatView.test.tsx` pins for a card with hidden characters on it.
 */
describe("ToolConfirmationBanner — hidden characters (§3.14)", () => {
  it("shows an override and a zero-width space in the arguments as escapes, and says how many", () => {
    const { section, box, escapes } = card({
      toolName: "shell_execute",
      toolArguments: {
        command: "rm -rf /tmp/\u{202E}fdp.exe /srv/allow\u{200B}ed",
      },
    });

    expect(raw(section.textContent ?? "")).toEqual([]);
    expect(box?.textContent).toBe(
      "rm -rf /tmp/<U+202E>fdp.exe /srv/allow<U+200B>ed",
    );
    expect(escapes).toEqual(["<U+202E>", "<U+200B>"]);
    const notice = screen.getByText(
      "Note: this request contains 2 hidden characters, shown as <U+…>.",
    );
    // Directly under the arguments it counts.
    expect(notice.previousElementSibling).toBe(box);
  });

  it("shows the tool name on one line, its hidden characters escaped and counted with the arguments'", () => {
    const { section, eyebrow, escapes } = card({
      toolName: "srv__rm\u{202E}fdp\u{200B}\nArguments: {}",
      toolArguments: { path: "/tmp/\u{200B}x" },
    });

    expect(raw(section.textContent ?? "", [0x0a])).toEqual([]);
    expect(eyebrow?.textContent).toBe(
      "Confirmation required · srv__rm<U+202E>fdp<U+200B><U+000A>Arguments: {}",
    );
    expect(escapes).toEqual(["<U+202E>", "<U+200B>", "<U+000A>", "<U+200B>"]);
    expect(
      screen.getByText(
        "Note: this request contains 4 hidden characters, shown as <U+…>.",
      ),
    ).toBeInTheDocument();
  });

  it("shows the asking agent's name escaped on one line, and counts it", () => {
    const { section } = card({
      toolName: "shell_execute",
      toolArguments: { command: "cargo tree -d" },
      agentName: "review\u{202E}_agent\u{3000}",
    });

    expect(raw(section.textContent ?? "")).toEqual([]);
    expect(section.lastElementChild?.textContent).toBe(
      "review<U+202E>_agent<U+3000> is blocked on this. Answer in the composer to continue.",
    );
    // Set apart as escapes, not drawn as the text a typed `<U+202E>` would be.
    expect(
      Array.from(
        section.lastElementChild?.querySelectorAll("[data-hidden-char]") ?? [],
        (node) => node.textContent,
      ),
    ).toEqual(["<U+202E>", "<U+3000>"]);
    expect(
      screen.getByText(
        "Note: this request contains 2 hidden characters, shown as <U+…>.",
      ),
    ).toBeInTheDocument();
  });

  it("shows a C1 control, a carriage return and a tag character by their code points", () => {
    const { section, eyebrow, box } = card({
      toolName: "shell\r_execute",
      toolArguments: { command: "printf 'a\u{9B}31m' done x\u{E0041}" },
    });

    expect(raw(section.textContent ?? "")).toEqual([]);
    expect(eyebrow?.textContent).toBe(
      "Confirmation required · shell<U+000D>_execute",
    );
    expect(box?.textContent).toBe("printf 'a<U+009B>31m' done x<U+E0041>");
    expect(
      screen.getByText(
        "Note: this request contains 3 hidden characters, shown as <U+…>.",
      ),
    ).toBeInTheDocument();
  });

  /**
   * A command's own line break drawn as a line break looks exactly like the
   * box's soft wrap: `rm -rf ~` would read as the tail of the echo before it
   * while the shell runs it as a second command. Such a value is shown as
   * JSON, `\n` written out, as every other approval surface shows it — and
   * JSON's own escape is not counted as hidden, there as here.
   */
  it("writes a command's own line break, tab or carriage return out, as JSON does", () => {
    const { box } = card({
      toolName: "shell_execute",
      toolArguments: {
        command: "echo please wait\nrm -rf ~\tdone\r",
      },
    });

    expect(box?.textContent?.split("\n")).toEqual([
      "{",
      '  "command": "echo please wait\\nrm -rf ~\\tdone\\r"',
      "}",
    ]);
    expect(screen.queryByText(/hidden character/)).toBeNull();
  });

  it("keeps the pretty-printed arguments' own lines", () => {
    const { box } = card({
      toolName: "file_write",
      toolArguments: { path: "/srv/allow\u{200B}ed", mode: "0755" },
    });

    expect(box?.textContent?.split("\n")).toEqual([
      "{",
      '  "path": "/srv/allow<U+200B>ed",',
      '  "mode": "0755"',
      "}",
    ]);
    expect(
      screen.getByText(
        "Note: this request contains 1 hidden character, shown as <U+…>.",
      ),
    ).toBeInTheDocument();
  });

  it("says nothing when nothing is hidden", () => {
    const { section, box, escapes } = card({
      toolName: "file_write",
      toolArguments: { path: "/tmp/日本語/שלום.txt" },
      agentName: "Alpaca",
    });

    expect(escapes).toEqual([]);
    expect(box?.textContent).toContain("/tmp/日本語/שלום.txt");
    expect(section.textContent).not.toMatch(/hidden character/);
  });

  /**
   * A run with no break opportunity — a URL's query string, base64, a padded
   * command — longer than the box used to run past its right edge, where an
   * overlay scrollbar draws nothing: the tail, and any escape in it, was off
   * screen while the notice still counted it. The card wraps such a run
   * anywhere (`overflow-wrap` inherits, so this reaches the eyebrow, the box
   * and the agent's name), and an escape is never split across two lines.
   */
  it("wraps a run too long for its line instead of running past the card's edge", () => {
    const { section, escapes } = card({
      toolName: `srv__${"x".repeat(80)}\u{200B}`,
      toolArguments: {
        url: `https://api.example.com/v1/export?session=${"a".repeat(120)}\u{202E}&dest=https%3A%2F%2Fevil.example`,
      },
      agentName: `agent_${"y".repeat(80)}\u{200B}`,
    });

    expect(section).toHaveClass("wrap-anywhere");
    expect(escapes).toEqual(["<U+200B>", "<U+202E>", "<U+200B>"]);
    for (const chip of section.querySelectorAll("[data-hidden-char]")) {
      expect(chip).toHaveClass("whitespace-nowrap");
    }
  });

  it("lays the arguments out left to right, isolated from the card around them", () => {
    const { box } = card({
      toolName: "file_write",
      toolArguments: { path: "/tmp/שלום/1/2" },
    });

    expect(box).toHaveAttribute("dir", "ltr");
    expect(box?.className).toContain("[unicode-bidi:isolate]");
  });

  it("shows what looks like markup, or like an escape, as the text it is", () => {
    const { section, box, escapes } = card({
      toolName: "shell_execute",
      toolArguments: { command: "<img src=x onerror=alert(1)> <U+202E>" },
    });

    expect(section.querySelector("img")).toBeNull();
    expect(box?.textContent).toBe("<img src=x onerror=alert(1)> <U+202E>");
    // Typed, not escaped: not set apart, and not counted.
    expect(escapes).toEqual([]);
    expect(section.textContent).not.toMatch(/hidden character/);
  });

  it("shows the daemon's own preview as it wrote it, uncounted", () => {
    const preview = "Replace your saved note “Editor”?\nYours: vim ❤\u{FE0F}.";
    const { section } = card({
      toolName: "memory_confirm",
      toolArguments: { id: "s", expected_revision: "r", target_revision: "t" },
      preview,
    });

    expect(
      screen.getByText((_, node) => node?.textContent === preview),
    ).toBeInTheDocument();
    expect(section.textContent).not.toMatch(/hidden character/);
  });
});

/**
 * With two prompts pending, the composer's Approve and Deny answer the oldest,
 * while the view scrolls to the newest card; two calls of one tool read alike
 * on the bar. The card the composer answers says so and is outlined in red,
 * like the composer's own answer box; every other card says it waits.
 */
describe("ToolConfirmationBanner — which card the composer answers", () => {
  it("marks the card the composer answers first when more than one is pending", () => {
    const { section } = card({
      toolName: "shell_execute",
      toolArguments: { command: "rm -rf ~/proj" },
      queue: { position: 1, total: 2 },
    });
    expect(section).toHaveAttribute("data-answered-next");
    expect(section).toHaveClass("border-red");
    expect(section.lastElementChild?.textContent).toBe(
      "The agent is blocked on this. Approve and Deny in the composer answer this card first — 1 of 2 pending, oldest first.",
    );
  });

  it("says a later card waits for the one outlined in red", () => {
    const { section } = card({
      toolName: "shell_execute",
      toolArguments: { command: "ls" },
      agentName: "researcher",
      queue: { position: 2, total: 2 },
    });
    expect(section).not.toHaveAttribute("data-answered-next");
    expect(section).not.toHaveClass("border-red");
    expect(section.lastElementChild?.textContent).toBe(
      "researcher is blocked on this. 2 of 2 pending — the composer answers the card outlined in red first.",
    );
  });

  it("reads as before when it is the only one", () => {
    const { section } = card({
      toolName: "shell_execute",
      toolArguments: { command: "ls" },
      queue: { position: 1, total: 1 },
    });
    expect(section).not.toHaveAttribute("data-answered-next");
    expect(section.lastElementChild?.textContent).toBe(
      "The agent is blocked on this. Answer in the composer to continue.",
    );
  });
});
