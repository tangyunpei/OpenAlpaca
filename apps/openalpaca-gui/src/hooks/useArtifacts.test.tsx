/**
 * An image's content URL carries the token in its query, so a view that drew
 * one is drawn again when the connection is read again (TOKEN-REREAD,
 * `tasks/pending-decisions.md` §1 _Rev 93_ (b)): the `<img>` then loads with
 * the token the daemon honours now, and `ImagePreview` gives the new URL its
 * own attempt even after the old one failed.
 */

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  bootstrapConnection,
  refreshConnection,
  resetConnection,
} from "@/lib/connection";
import { fixtureHost } from "@/lib/host/fixture";

import { useArtifactContentUrl } from "./useArtifacts";

function serving(token: string): void {
  fixtureHost!.answerConnection({
    baseUrl: "http://127.0.0.1:51823",
    token,
    instanceId: "i-1",
    daemonPath: null,
  });
}

beforeEach(() => {
  resetConnection();
});

afterEach(() => {
  fixtureHost!.reset();
});

describe("useArtifactContentUrl", () => {
  it("is null before the connection is read, and for no artifact", async () => {
    const { result, rerender } = renderHook(
      ({ id }: { id: string | null }) => useArtifactContentUrl(id),
      { initialProps: { id: "a 1" as string | null } },
    );
    expect(result.current).toBeNull();

    serving("t1");
    await act(async () => {
      await bootstrapConnection();
    });
    rerender({ id: null });
    expect(result.current).toBeNull();
  });

  it("is drawn again with the token the connection was read again for", async () => {
    serving("t1");
    await bootstrapConnection();
    const { result } = renderHook(() => useArtifactContentUrl("a 1"));
    expect(result.current).toBe(
      "http://127.0.0.1:51823/v1/artifacts/a%201/content?token=t1",
    );

    serving("t2");
    await act(async () => {
      await refreshConnection();
    });
    expect(result.current).toBe(
      "http://127.0.0.1:51823/v1/artifacts/a%201/content?token=t2",
    );

    act(() => resetConnection());
    expect(result.current).toBeNull();
  });
});
