import "@testing-library/jest-dom/vitest";

import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach } from "vitest";

/**
 * The window every test renders into, unless it says otherwise (T2).
 *
 * jsdom defaults to 1024×768, which since T2 is a *narrow* window: the chat
 * view collapses its conversation column there, exactly as it would on a
 * half-screen split. That is real behaviour and it has its own tests — but it
 * is not the window the rest of the suite means, so the default here is a
 * desktop one. A test about a narrow window sets `window.innerWidth` itself
 * and this restores it afterwards.
 */
export const DEFAULT_TEST_WINDOW_WIDTH = 1600;
export const DEFAULT_TEST_WINDOW_HEIGHT = 1000;

beforeEach(() => {
  window.innerWidth = DEFAULT_TEST_WINDOW_WIDTH;
  window.innerHeight = DEFAULT_TEST_WINDOW_HEIGHT;
});

afterEach(() => {
  cleanup();
});
