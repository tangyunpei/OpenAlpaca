/**
 * REST API client for file upload/download endpoints.
 */

import { get } from "svelte/store";
import { connectionInfo, type ConnectionInfo } from "../daemon";
import type { FileUploadResponse, FileAsset } from "../types";

async function ensureConnection(): Promise<ConnectionInfo> {
  const conn = get(connectionInfo);
  if (!conn) throw new Error("Not connected to daemon");
  return conn;
}

/** POST /v1/files/upload — Upload a file via multipart form data. */
export async function uploadFile(
  file: File,
  onProgress?: (loaded: number, total: number) => void,
): Promise<FileUploadResponse> {
  const conn = await ensureConnection();
  const formData = new FormData();
  formData.append("file", file);

  if (onProgress) {
    return new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open("POST", `${conn.baseUrl}/v1/files/upload`);
      xhr.setRequestHeader("Authorization", `Bearer ${conn.token}`);

      xhr.upload.onprogress = (e) => {
        if (e.lengthComputable) onProgress(e.loaded, e.total);
      };

      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) {
          try { resolve(JSON.parse(xhr.responseText)); }
          catch { reject(new Error("Failed to parse upload response")); }
        } else {
          try {
            const data = JSON.parse(xhr.responseText);
            reject(new Error(data.error?.message || `Upload failed: ${xhr.statusText}`));
          } catch { reject(new Error(`Upload failed: ${xhr.statusText}`)); }
        }
      };

      xhr.onerror = () => reject(new Error("Upload network error"));
      xhr.onabort = () => reject(new Error("Upload aborted"));
      xhr.send(formData);
    });
  }

  const response = await fetch(`${conn.baseUrl}/v1/files/upload`, {
    method: "POST",
    headers: { Authorization: `Bearer ${conn.token}` },
    body: formData,
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Upload failed: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/files/{id} — Get file metadata. */
export async function getFileMetadata(id: string): Promise<FileAsset> {
  const conn = await ensureConnection();
  const response = await fetch(`${conn.baseUrl}/v1/files/${encodeURIComponent(id)}`, {
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Failed to fetch file: ${response.statusText}`);
  }
  return await response.json();
}

/** GET /v1/files/{id}/content — Download file as Blob. */
export async function downloadFile(id: string): Promise<Blob> {
  const conn = await ensureConnection();
  const response = await fetch(
    `${conn.baseUrl}/v1/files/${encodeURIComponent(id)}/content`,
    { headers: { Authorization: `Bearer ${conn.token}` } },
  );

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Download failed: ${response.statusText}`);
  }
  return await response.blob();
}
