/**
 * `GET|PUT /v1/persona/user` — the `USER.md` editor's client (design §6
 * *USER.md*, §7a).
 *
 * The revision is the file's SHA-256, served as `ETag` and sent back as
 * `If-Match`; the body carries only `{content}`. So the one thing this client
 * must never get wrong is carrying the tag from the read to the write.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { resetConnection } from "@/lib/connection";
import { ApiError } from "@/lib/http";

import {
  getPersonaUser,
  isPersonaConflict,
  putPersonaUser,
  sha256Hex,
} from "./persona";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => ({
    baseUrl: "http://127.0.0.1:9999",
    token: "test-token",
    instanceId: "7f3a1122",
  })),
}));

interface Recorded {
  url: URL;
  method: string;
  body: unknown;
  headers: Headers;
}

let requests: Recorded[] = [];
let reply: () => Response;

const CONTENT = "# About the user\n\nCall me Pei.\n";

beforeEach(() => {
  requests = [];
  resetConnection();
  reply = () => new Response(JSON.stringify({ content: CONTENT }));
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: unknown, init?: RequestInit) => {
      requests.push({
        url: new URL(String(input)),
        method: init?.method ?? "GET",
        body: typeof init?.body === "string" ? JSON.parse(init.body) : null,
        headers: new Headers(init?.headers),
      });
      return reply();
    }),
  );
});

describe("getPersonaUser", () => {
  it("takes the revision from the quoted ETag", async () => {
    const tag = "e".repeat(64);
    reply = () =>
      new Response(JSON.stringify({ content: CONTENT }), {
        headers: { ETag: `"${tag}"` },
      });
    const read = await getPersonaUser();
    expect(requests[0]?.url.pathname).toBe("/v1/persona/user");
    expect(read).toEqual({ content: CONTENT, etag: tag });
  });

  /**
   * The tag is the SHA-256 of the file's UTF-8 bytes and the body is those
   * bytes as a string, so a response whose `ETag` the webview did not expose
   * still yields the same revision — and a write that carries it is not
   * refused as stale for a reason the owner could never see.
   */
  it("derives the same revision from the content when no ETag is readable", async () => {
    const read = await getPersonaUser();
    expect(read.etag).toBe(await sha256Hex(CONTENT));
    expect(read.etag).toMatch(/^[0-9a-f]{64}$/);
  });
});

describe("sha256Hex", () => {
  it("hashes the UTF-8 bytes, CJK included", async () => {
    expect(await sha256Hex("")).toBe(
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
    // `printf '测试' | shasum -a 256`
    expect(await sha256Hex("测试")).toBe(
      "6aa8f49cc992dfd75a114269ed26de0ad6d4e7d7a70d9c8afb3d7a57a88a73ed",
    );
  });
});

describe("putPersonaUser", () => {
  it("sends the whole document with the revision it edited in If-Match", async () => {
    const tag = "f".repeat(64);
    reply = () => new Response(JSON.stringify({ etag: "1".repeat(64) }));
    const written = await putPersonaUser("# About\n", tag);
    const request = requests[0];
    expect(request?.method).toBe("PUT");
    expect(request?.url.pathname).toBe("/v1/persona/user");
    expect(request?.headers.get("If-Match")).toBe(`"${tag}"`);
    expect(request?.body).toEqual({ content: "# About\n" });
    expect(written.etag).toBe("1".repeat(64));
  });

  it("names the stale-revision refusal apart from every other one", async () => {
    reply = () =>
      new Response(
        JSON.stringify({
          error: {
            code: "PERSONA_REVISION_CONFLICT",
            message: "USER.md changed since you opened it",
          },
        }),
        { status: 409 },
      );
    const error = await putPersonaUser("x", "0".repeat(64)).then(
      () => null,
      (e: unknown) => e,
    );
    expect(isPersonaConflict(error)).toBe(true);
    expect(isPersonaConflict(new ApiError("bad", 400, "PERSONA_INVALID"))).toBe(
      false,
    );
  });
});
