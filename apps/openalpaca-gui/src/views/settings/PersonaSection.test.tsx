/**
 * Settings → Persona — the `USER.md` editor (memory redesign, design §6
 * *USER.md*, §7a `GET|PUT /v1/persona/user`; MD-3, MD-9).
 *
 * Over the real data layer: only the Tauri discovery command and `fetch` are
 * doubled, and `fetch` holds one file and one revision the way the daemon
 * does — `PUT` checks `If-Match` against it, answers `409
 * PERSONA_REVISION_CONFLICT` when it moved and `400 PERSONA_INVALID` for a body
 * `update_persona` would refuse.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";

import { PERSONA_CONFLICT_COPY, PersonaSection } from "./PersonaSection";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

interface Recorded {
  method: string;
  body: { content?: string } | null;
  ifMatch: string | null;
}

let requests: Recorded[] = [];
let file = "";
let revision = 0;
let invalid: string | null = null;

function etag(): string {
  return revision.toString(16).padStart(64, "0");
}

function json(payload: unknown, status = 200, headers: HeadersInit = {}) {
  return new Response(JSON.stringify(payload), { status, headers });
}

/** Another writer — the onboarding `update_persona`, the CLI — edits the file. */
function writeBehindTheEditorsBack(content: string): void {
  file = content;
  revision += 1;
}

beforeEach(() => {
  requests = [];
  file = "# About the user\n\nCall me Pei.\n";
  revision = 1;
  invalid = null;
  resetConnection();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (_input: unknown, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      const headers = new Headers(init?.headers);
      const body =
        typeof init?.body === "string"
          ? (JSON.parse(init.body) as { content?: string })
          : null;
      const ifMatch = headers.get("If-Match");
      requests.push({ method, body, ifMatch });
      if (method === "GET") {
        return json({ content: file }, 200, { ETag: `"${etag()}"` });
      }
      if (ifMatch === null) {
        return json(
          {
            error: {
              code: "PERSONA_IF_MATCH_REQUIRED",
              message: "send If-Match",
            },
          },
          428,
        );
      }
      if (invalid !== null) {
        return json(
          { error: { code: "PERSONA_INVALID", message: invalid } },
          400,
        );
      }
      if (ifMatch.replace(/"/g, "") !== etag()) {
        return json(
          {
            error: {
              code: "PERSONA_REVISION_CONFLICT",
              message: `USER.md changed: you edited ${ifMatch}, the current revision is ${etag()}`,
            },
          },
          409,
          { ETag: `"${etag()}"` },
        );
      }
      writeBehindTheEditorsBack(body?.content ?? "");
      return json({ etag: etag() }, 200, { ETag: `"${etag()}"` });
    }),
  );
});

function renderSection() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={client}>
      <PersonaSection />
    </QueryClientProvider>,
  );
}

function puts(): Recorded[] {
  return requests.filter((r) => r.method === "PUT");
}

describe("PersonaSection", () => {
  it("opens USER.md as it is on disk, with nothing to save yet", async () => {
    renderSection();
    const editor = await screen.findByLabelText("USER.md");
    expect(editor).toHaveValue("# About the user\n\nCall me Pei.\n");
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  });

  it("saves the whole document under the revision it opened", async () => {
    renderSection();
    const user = userEvent.setup();
    const editor = await screen.findByLabelText("USER.md");

    await user.type(editor, "Prefers short answers.\n");
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(puts()[0]?.ifMatch).toBe(`"${"1".padStart(64, "0")}"`);
    expect(puts()[0]?.body).toEqual({
      content: "# About the user\n\nCall me Pei.\nPrefers short answers.\n",
    });
    expect(file).toBe(
      "# About the user\n\nCall me Pei.\nPrefers short answers.\n",
    );
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Save" })).toBeDisabled(),
    );
  });

  it("keeps the draft and says so when USER.md changed underneath the editor", async () => {
    renderSection();
    const user = userEvent.setup();
    const editor = await screen.findByLabelText("USER.md");
    await user.type(editor, "Prefers short answers.\n");

    writeBehindTheEditorsBack(
      "# About the user\n\nCall me Pei. Speaks Mandarin.\n",
    );
    await user.click(screen.getByRole("button", { name: "Save" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(PERSONA_CONFLICT_COPY);
    expect(PERSONA_CONFLICT_COPY).toBe(
      "USER.md changed since you opened it. Reload to see the current version — your text is kept.",
    );
    expect(screen.getByLabelText("USER.md")).toHaveValue(
      "# About the user\n\nCall me Pei.\nPrefers short answers.\n",
    );
    // The conflict is said in this window's words, not as a failed save.
    expect(screen.queryByText(/Could not save/)).toBeNull();
    expect(file).toBe("# About the user\n\nCall me Pei. Speaks Mandarin.\n");

    await user.click(within(alert).getByRole("button", { name: "Reload" }));
    expect(
      await screen.findByRole("region", { name: "Current version" }),
    ).toHaveTextContent("Speaks Mandarin.");
    expect(screen.getByLabelText("USER.md")).toHaveValue(
      "# About the user\n\nCall me Pei.\nPrefers short answers.\n",
    );

    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(2));
    expect(puts()[1]?.ifMatch).toBe(`"${"2".padStart(64, "0")}"`);
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(file).toBe(
      "# About the user\n\nCall me Pei.\nPrefers short answers.\n",
    );
  });

  it("shows the daemon's own reason for a document it refuses", async () => {
    invalid = "USER.md must keep its top-level heading";
    renderSection();
    const user = userEvent.setup();
    const editor = await screen.findByLabelText("USER.md");
    await user.clear(editor);
    await user.type(editor, "no heading");
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(
      await screen.findByText(/USER\.md must keep its top-level heading/),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("USER.md")).toHaveValue("no heading");
  });
});
