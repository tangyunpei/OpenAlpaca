import { describe, expect, it } from "vitest";

import { escapeConfirmation } from "./confirmation-escape";

/**
 * One count for a pending request, whichever surface shows it: the card
 * draws the three fields and says how many, and the bar, the palette's row
 * and the run card show the name alone and say the same number.
 */
describe("escapeConfirmation", () => {
  it("counts the name, the arguments as the card lays them out, and the agent", () => {
    const escaped = escapeConfirmation({
      toolName: "srv__rm\u{202E}fdp",
      toolArguments: { path: "/srv/allow\u{200B}ed", mode: "0755" },
      agentName: "review\tagent",
    });

    expect(escaped.name.text).toBe("srv__rm<U+202E>fdp");
    expect(escaped.command.text).toBe(
      '{\n  "path": "/srv/allow<U+200B>ed",\n  "mode": "0755"\n}',
    );
    expect(escaped.agent?.text).toBe("review<U+0009>agent");
    expect(escaped.hidden).toBe(3);
  });

  it("takes no agent as nothing to count", () => {
    const escaped = escapeConfirmation({
      toolName: "shell_execute",
      toolArguments: { command: "ls\u{200B}" },
      agentName: null,
    });
    expect(escaped.agent).toBeNull();
    expect(escaped.hidden).toBe(1);
  });
});
