import { describe, expect, it } from "vitest";

import {
  artifactKindForMime,
  fileAbbr,
  languageFromName,
  toFileKind,
} from "./file-kind";

describe("file kinds", () => {
  it("maps the proposed artifact API's kinds onto the design's seven", () => {
    expect(toFileKind("markdown")).toBe("md");
    expect(toFileKind("terminal")).toBe("term");
    expect(toFileKind("code")).toBe("code");
    // No badge exists for opaque bytes; tool output is the nearest truth.
    expect(toFileKind("binary")).toBe("term");
  });

  it("uses the fixed abbreviation for every non-code kind", () => {
    expect(fileAbbr("md")).toBe("MD");
    expect(fileAbbr("plan")).toBe("PLN");
    expect(fileAbbr("term")).toBe("OUT");
    expect(fileAbbr("table")).toBe("CSV");
    expect(fileAbbr("html")).toBe("WEB");
    expect(fileAbbr("image")).toBe("IMG");
    // Language is ignored for non-code kinds.
    expect(fileAbbr("md", "rs")).toBe("MD");
  });

  it("derives the code badge from the language", () => {
    expect(fileAbbr("code", "rs")).toBe("RS");
    expect(fileAbbr("code", "rust")).toBe("RS");
    expect(fileAbbr("code", ".TS")).toBe("TS");
    expect(fileAbbr("code", "kotlin")).toBe("KOT");
  });

  it("says SRC rather than guessing when the language is unknown", () => {
    expect(fileAbbr("code")).toBe("SRC");
    expect(fileAbbr("code", null)).toBe("SRC");
    expect(fileAbbr("code", "")).toBe("SRC");
  });

  // The rows of `artifact_kind_for_mime_follows_the_r25_table`
  // (`crates/openalpaca_storage/src/models/artifact.rs`): the kind the daemon
  // stores for an upload, which the Library badges (G3a).
  it.each([
    ["image/png", "image"],
    ["image/svg+xml", "image"],
    ["text/html", "html"],
    ["application/xhtml+xml", "html"],
    ["text/markdown", "markdown"],
    ["text/x-markdown", "markdown"],
    ["text/csv", "table"],
    ["text/tab-separated-values", "table"],
    ["text/plain", "code"],
    ["text/x-python", "code"],
    ["application/json", "code"],
    ["application/toml", "code"],
    ["application/pdf", "binary"],
    ["application/octet-stream", "binary"],
    ["audio/mpeg", "binary"],
    ["", "binary"],
    ["text/HTML; charset=utf-8", "html"],
  ] as const)("projects %s to the daemon's %s", (mime, kind) => {
    expect(artifactKindForMime(mime)).toBe(kind);
  });

  it("reads a language off a filename", () => {
    expect(languageFromName("findings.md")).toBe("md");
    expect(languageFromName("main.RS")).toBe("rs");
    expect(languageFromName("Makefile")).toBeNull();
    expect(languageFromName("trailing.")).toBeNull();
  });
});
