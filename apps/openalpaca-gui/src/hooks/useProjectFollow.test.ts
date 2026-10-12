import { describe, expect, it } from "vitest";

import type { Project } from "@/lib/api/workspaces";

import { followedToast, projectToFollow } from "./useProjectFollow";

const MOVED: Project = {
  id: "6f1c2a3b-4d5e-4f60-8a7b-9c0d1e2f3a4b",
  root: "/Users/dev/work/app",
  name: "-Users-dev-work-app",
  missing: false,
};

const OTHER: Project = {
  id: "c3d4e5f6-a7b8-4c9d-8e0f-1a2b3c4d5e6f",
  root: "/Users/dev/app",
  name: "-Users-dev-app",
  missing: false,
};

describe("projectToFollow (R22)", () => {
  it("is the project with the held id when it is listed at another root", () => {
    expect(projectToFollow("/Users/dev/app", MOVED.id, [OTHER, MOVED])).toBe(
      MOVED,
    );
  });

  it("is nothing when the project is still at the held root", () => {
    expect(projectToFollow(MOVED.root, MOVED.id, [OTHER, MOVED])).toBeNull();
  });

  it("is nothing when no project with the held id is listed, whatever holds the root now", () => {
    // The project's data was deleted; another project registered at the
    // same root since is not this window's project.
    expect(projectToFollow("/Users/dev/app", MOVED.id, [OTHER])).toBeNull();
    expect(projectToFollow("/Users/dev/app", MOVED.id, [])).toBeNull();
  });
});

describe("followedToast", () => {
  it("names the new root and what goes there", () => {
    expect(followedToast(MOVED.root)).toBe(
      "This window's project was re-pointed to /Users/dev/work/app: its turns go there now.",
    );
  });
});
