/**
 * `/v1/files*` — metadata, download, and host-side open.
 *
 * Reading content for a *preview* lives in `api/artifacts` instead: the content
 * routes take `?token=` inline, so `artifactContentUrl` hands a browser a URL
 * it can load and `getArtifactText` reads the characters. `downloadFile` here
 * stays the blob a viewer saves.
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
