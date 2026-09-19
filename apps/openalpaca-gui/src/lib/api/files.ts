/**
 * `/v1/files*` — upload, metadata, download, and host-side open.
 *
 * Reading content for a *preview* lives in `api/artifacts` instead: the content
 * routes take `?token=` inline, so `artifactContentUrl` hands a browser a URL
 * it can load and `getArtifactText` reads the characters. `downloadFile` here
 * stays the blob a viewer saves.
 */

import { ApiError, apiFetch, apiFetchBlob } from "../http";
import { workspaceHeader } from "../workspace-header";
import type { FileAsset, FileOpenResponse, FileUploadResponse } from "./types";

/**
 * `POST /v1/files/upload` — one file, multipart (U5).
 *
 * The shape is the CLI's, field for field (`DaemonClient::upload_file`): a
 * single part named `file`, carrying the picked name and the browser's own
 * media type. The daemon reads `multipart.next_field()`, so it is the *first*
 * part that counts, and it validates the declared type against the file's
 * magic bytes — which is why nothing here guesses a MIME type the picker did
 * not give. A file the browser cannot type arrives as
 * `application/octet-stream` and the daemon refuses it by name; inventing a
 * type here would only move that refusal somewhere less honest.
 *
 * `x-workspace-path` travels with it for the same reason a chat turn carries
 * it: the header is what picks the store the bytes land in (D2), so an upload
 * for a turn in a project belongs in that project's store.
 */
export async function uploadFile(
  file: File,
  options: { workspacePath?: string | null; signal?: AbortSignal } = {},
): Promise<FileUploadResponse> {
  const form = new FormData();
  form.append("file", file, file.name);
  return await apiFetch<FileUploadResponse>("/v1/files/upload", {
    method: "POST",
    formData: form,
    headers: workspaceHeader(options.workspacePath),
    signal: options.signal,
  });
}

/**
 * What a failed upload says on the chip — the **daemon's** own sentence
 * wherever there is one (`UNSUPPORTED_MIME`, `MIME_MISMATCH`, the size cap).
 *
 * Only a request that never reached the daemon gets a sentence of ours, and it
 * says exactly that rather than dressing a transport failure as a refusal.
 */
export function uploadErrorMessage(cause: unknown): string {
  if (cause instanceof ApiError) {
    return cause.isTransport ? "Could not reach the daemon" : cause.message;
  }
  return cause instanceof Error ? cause.message : "Upload failed";
}

/** `GET /v1/files/{id}` */
export async function getFileMetadata(
  id: string,
  signal?: AbortSignal,
): Promise<FileAsset> {
  return await apiFetch<FileAsset>(`/v1/files/${encodeURIComponent(id)}`, {
    signal,
  });
}

/** `GET /v1/files/{id}/content` as a `Blob`. */
export async function downloadFile(
  id: string,
  signal?: AbortSignal,
): Promise<Blob> {
  return await apiFetchBlob(`/v1/files/${encodeURIComponent(id)}/content`, {
    signal,
  });
}

/**
 * `POST /v1/files/{id}/open` — opens with the daemon host's default app. Note
 * this *opens*, it does not reveal in Finder; revealing needs a Tauri command
 * that does not exist yet.
 */
export async function openFileWithSystemDefault(
  id: string,
): Promise<FileOpenResponse> {
  return await apiFetch<FileOpenResponse>(
    `/v1/files/${encodeURIComponent(id)}/open`,
    {
      method: "POST",
    },
  );
}
