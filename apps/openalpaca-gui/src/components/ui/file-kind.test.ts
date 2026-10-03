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

  // Every row of `artifact_kind_for_mime_follows_the_r25_table` and
  // `artifact_kind_for_mime_ignores_parameters_and_case`
  // (`crates/openalpaca_storage/src/models/artifact.rs`): the kind the daemon
  // stores for an upload, which the Library badges (G3a).
  it.each([
    ["image/png", "image"],
    ["image/jpeg", "image"],
    ["image/svg+xml", "image"],
    ["text/html", "html"],
    ["application/xhtml+xml", "html"],
    ["text/markdown", "markdown"],
    ["text/x-markdown", "markdown"],
    ["text/csv", "table"],
    ["text/tab-separated-values", "table"],
    ["text/plain", "code"],
    ["text/x-python", "code"],
    ["text/yaml", "code"],
    ["application/json", "code"],
    ["application/yaml", "code"],
    ["application/toml", "code"],
    ["application/xml", "code"],
    ["application/javascript", "code"],
    ["application/pdf", "binary"],
    ["application/zip", "binary"],
    ["application/octet-stream", "binary"],
    ["audio/mpeg", "binary"],
    ["", "binary"],
    ["text/HTML; charset=utf-8", "html"],
    ["  text/markdown  ", "markdown"],
  ] as const)("projects %s to the daemon's %s", (mime, kind) => {
    expect(artifactKindForMime(mime)).toBe(kind);
  });

  // The port's `CODE_MIME_TYPES` is the daemon's list, kept equal by hand:
  // every entry of the Rust `CODE_MIME_TYPES` is stored as `code`.
  it.each([
    "application/json",
    "application/ld+json",
    "application/yaml",
    "application/x-yaml",
    "application/toml",
    "application/x-toml",
    "application/xml",
    "application/javascript",
    "application/ecmascript",
    "application/typescript",
    "application/sql",
    "application/x-python-code",
    "application/x-sh",
  ])("stores %s as code, as the daemon's list does", (mime) => {
    expect(artifactKindForMime(mime)).toBe("code");
  });

  it("reads a language off a filename", () => {
    expect(languageFromName("findings.md")).toBe("md");
    expect(languageFromName("main.RS")).toBe("rs");
    expect(languageFromName("Makefile")).toBeNull();
    expect(languageFromName("trailing.")).toBeNull();
  });
});
