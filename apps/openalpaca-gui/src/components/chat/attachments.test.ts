/**
 * The draft-attachment contract (U5), pinned as a table.
 *
 * What travels and what does not is the load-bearing half: a chip the daemon
 * refused must never reach `POST /v1/chat`, and the cap's sentence has to be
 * the daemon's own so the UI's refusal and the wire's read the same.
 */

import { describe, expect, it } from "vitest";

import {
  MAX_ATTACHMENTS,
  hasUploadInFlight,
  readyAttachmentRefs,
  tooManyAttachmentsMessage,
  type DraftAttachment,
} from "./attachments";

function chip(overrides: Partial<DraftAttachment> = {}): DraftAttachment {
  return {
    key: "att-1",
    name: "notes.txt",
    size: 12,
    state: "ready",
    fileId: "file-1",
    error: null,
    ...overrides,
  };
}

describe("readyAttachmentRefs", () => {
  it("carries only the chips the daemon gave an id, in pick order", () => {
    expect(
      readyAttachmentRefs([
        chip({ key: "a", fileId: "file-a" }),
        chip({ key: "b", state: "uploading", fileId: null }),
        chip({
          key: "c",
          state: "failed",
          fileId: null,
          error: "MIME type 'application/zip' is not allowed",
        }),
        chip({ key: "d", fileId: "file-d" }),
      ]),
    ).toEqual([{ file_id: "file-a" }, { file_id: "file-d" }]);
  });

  it("is empty when nothing has landed yet", () => {
    expect(
      readyAttachmentRefs([chip({ state: "uploading", fileId: null })]),
    ).toEqual([]);
  });
});

describe("hasUploadInFlight", () => {
  it("is true only while something is still uploading", () => {
    expect(hasUploadInFlight([])).toBe(false);
    expect(hasUploadInFlight([chip()])).toBe(false);
    expect(hasUploadInFlight([chip({ state: "failed" })])).toBe(false);
    expect(
      hasUploadInFlight([chip(), chip({ key: "b", state: "uploading" })]),
    ).toBe(true);
  });
});

describe("tooManyAttachmentsMessage", () => {
  /** Word for word the daemon's `TOO_MANY_ATTACHMENTS` body. */
  it("reads exactly as the daemon's own refusal", () => {
    expect(tooManyAttachmentsMessage(11)).toBe(
      "Too many attachments: 11 provided, maximum is 10",
    );
    expect(MAX_ATTACHMENTS).toBe(10);
  });
});
