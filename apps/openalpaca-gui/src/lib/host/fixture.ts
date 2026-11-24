/**
 * FixtureHost (Q07): the host of the tests and of a plain browser session,
 * in a development build only (a production build folds it away).
 *
 * It starts UNANSWERED: every command then refuses exactly as a missing
 * Tauri bridge does, "Tauri command `<command>` failed" with no `kind`, so
 * a test or a plain `bun run dev` meets what it always met. With
 * `VITE_OPENALPACA_HOST=fixture`, outside the test run, it starts with a
 * known connection instead (to `127.0.0.1:9999`, where no daemon is).
 *
 * Its own API sets its answers: per command a function `(...args) => raw`
 * (or a promise of one) that throws or rejects to refuse, a one-shot answer
 * queued ahead of the standing one, both connection commands at once, and
 * whether Reveal is drawn. Every answer and every refusal passes through
 * `lib/host/answer.ts`, exactly as the shell's does, so the raw value a
 * fixture is given — a `ConnectionInfo`-shaped object, a refusal row of
 * the shell's, a bare string, an unexpected payload — meets the same
 * result. `reset()` returns it to its start, before every test
 * (`vitest.setup.ts`).
 *
 * It imports `./answer` only, so the test setup's import of it pulls no
 * module a test file mocks.
 */

import { connectionAnswer, logTailAnswer, stopReportAnswer } from "./answer";
import type { HostBridge } from "./types";

/** The arguments of each command the fixture answers. */
interface FixtureArgs {
  ensureDaemonRunning: [];
  getConnectionInfo: [];
  readDaemonLogTail: [lines: number];
  awaitDaemonStopped: [];
}

export type FixtureCommand = keyof FixtureArgs;

/** A command's raw answer, as the shell would give it; throw to refuse. */
export type FixtureAnswer<C extends FixtureCommand> = (
  ...args: FixtureArgs[C]
) => unknown;

export interface FixtureHost extends HostBridge {
  readonly kind: "fixture";
  /** Answer `command` with `answer` from now on. */
  answer<C extends FixtureCommand>(command: C, answer: FixtureAnswer<C>): void;
  /** Answer `command`'s next call with `answer`, ahead of the standing one. */
  answerOnce<C extends FixtureCommand>(
    command: C,
    answer: FixtureAnswer<C>,
  ): void;
  /** Answer both connection commands with `raw` from now on. */
  answerConnection(raw: unknown): void;
  /** Whether this window draws Reveal. */
  setReveal(reveal: boolean): void;
  /** Back to the start: unanswered, or the known connection (see above). */
  reset(): void;
}

function createFixtureHost(): FixtureHost {
  const standing = new Map<FixtureCommand, FixtureAnswer<never>>();
  const once = new Map<FixtureCommand, Array<FixtureAnswer<never>>>();
  let reveal = false;

  /** `command`'s raw answer to `args`; thrown when it has none. */
  function ask<C extends FixtureCommand>(
    command: C,
    ...args: FixtureArgs[C]
  ): unknown {
    const answer = (once.get(command)?.shift() ?? standing.get(command)) as
      FixtureAnswer<C> | undefined;
    if (answer === undefined) {
      throw new Error(`the fixture host has no answer for ${command}`);
    }
    return answer(...args);
  }

  const host: FixtureHost = {
    kind: "fixture",
    get reveal() {
      return reveal;
    },
    dialog: null,
    window: null,
    ensureDaemonRunning: () =>
      connectionAnswer("ensure_daemon_running", () =>
        ask("ensureDaemonRunning"),
      ),
    getConnectionInfo: () =>
      connectionAnswer("get_connection_info", () => ask("getConnectionInfo")),
    readDaemonLogTail: (lines) =>
      logTailAnswer(() => ask("readDaemonLogTail", lines)),
    awaitDaemonStopped: () => stopReportAnswer(() => ask("awaitDaemonStopped")),
    answer(command, answer) {
      standing.set(command, answer as FixtureAnswer<never>);
    },
    answerOnce(command, answer) {
      const queue = once.get(command) ?? [];
      queue.push(answer as FixtureAnswer<never>);
      once.set(command, queue);
    },
    answerConnection(raw) {
      host.answer("ensureDaemonRunning", () => raw);
      host.answer("getConnectionInfo", () => raw);
    },
    setReveal(next) {
      reveal = next;
    },
    reset() {
      standing.clear();
      once.clear();
      reveal = false;
      if (
        import.meta.env.VITE_OPENALPACA_HOST === "fixture" &&
        import.meta.env.MODE !== "test"
      ) {
        host.answerConnection({
          baseUrl: "http://127.0.0.1:9999",
          token: "test-token",
          instanceId: "7f3a1122",
        });
        host.answer("readDaemonLogTail", () => "");
        host.answer("awaitDaemonStopped", () => ({
          outcome: "stopped",
          pid: null,
          waitedMs: 0,
        }));
      }
    },
  };
  host.reset();
  return host;
}

/**
 * The fixture, in a development build; `null` in a production one, which
 * folds the factory and its strings away.
 */
export const fixtureHost: FixtureHost | null = import.meta.env.DEV
  ? createFixtureHost()
  : null;
