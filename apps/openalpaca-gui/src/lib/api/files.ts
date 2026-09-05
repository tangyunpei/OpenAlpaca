/**
 * `/v1/files*` — metadata, download, and host-side open.
 *
 * Inline preview of file CONTENT is deliberately absent: `/content` sits behind
 * the auth middleware, so `<img src>`/`<iframe src>` cannot load it (API_MAP
 * GAP-11). The `fetch → blob → createObjectURL` workaround lands together with
 * the artifact API (GAP-04), which is what would supply a preview source.
 */

import { apiFetch, apiFetchBlob } from "../http";
import type { FileAsset, FileOpenResponse } from "./types";

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
 * A blob object URL for `<img>`/`<iframe>`, plus its revoke function.
 * Purely a workaround for GAP-11 — delete this once the content route accepts
 * `?token=`.
 */
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
