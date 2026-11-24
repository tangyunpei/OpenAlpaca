/**
 * FixtureHost's own API (Q07, `lib/host/fixture.ts`): what `reset()` takes
 * back. `vitest.setup.ts` resets the fixture before every test, so an
 * answer one test leaves unused must not reach the next.
 */

import { describe, expect, it } from "vitest";

import { ConnectionError } from "./answer";
import { fixtureHost } from "./fixture";

const answer = {
  baseUrl: "http://127.0.0.1:9999",
  token: "test-token",
  instanceId: "7f3a1122",
};

describe("FixtureHost's reset", () => {
  it("drops a one-shot answer left unused, as well as the standing one", async () => {
    const fixture = fixtureHost!;
    fixture.answer("getConnectionInfo", () => answer);
    fixture.answerOnce("getConnectionInfo", () => answer);

    fixture.reset();

    const refusal = await fixture.getConnectionInfo().then(
      () => null,
      (error: unknown) => error,
    );
    expect(refusal).toBeInstanceOf(ConnectionError);
    expect((refusal as ConnectionError).message).toBe(
      "Tauri command `get_connection_info` failed",
    );
  });
});
