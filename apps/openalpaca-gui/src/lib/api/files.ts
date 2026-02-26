/**
 * REST API client for file upload/download endpoints.
 */

import { get } from "svelte/store";
import { save } from "@tauri-apps/plugin-dialog";
import { writeFile } from "@tauri-apps/plugin-fs";
import { connectionInfo, type ConnectionInfo } from "../daemon";
import type { FileUploadResponse, FileAsset, FileOpenResponse } from "../types";

let systemOpenEndpointUnavailable = false;
let systemOpenEndpointInstanceId: string | null = null;

export type SaveFileWithDialogResult = "saved" | "cancelled" | "unavailable";

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

/** POST /v1/files/{id}/open — Open file with system default app on daemon host. */
export async function openFileWithSystemDefault(id: string): Promise<FileOpenResponse> {
  const conn = await ensureConnection();
  if (systemOpenEndpointInstanceId !== conn.instanceId) {
    systemOpenEndpointInstanceId = conn.instanceId;
    systemOpenEndpointUnavailable = false;
  }

  if (systemOpenEndpointUnavailable) {
    throw new Error("System open endpoint unavailable");
  }

  const response = await fetch(`${conn.baseUrl}/v1/files/${encodeURIComponent(id)}/open`, {
    method: "POST",
    headers: { Authorization: `Bearer ${conn.token}` },
  });

  if (response.status === 404) {
    // Route may be unavailable on an older daemon build; avoid retrying every click.
    systemOpenEndpointUnavailable = true;
    throw new Error("System open endpoint unavailable");
  }

  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data.error?.message || `Open failed: ${response.statusText}`);
  }

  return await response.json();
}

function isPluginUnavailableError(error: unknown): boolean {
  if (!error) return false;
  const message = error instanceof Error ? error.message : String(error);
  const normalized = message.toLowerCase();
  return (
    normalized.includes("plugin") ||
    normalized.includes("not available") ||
    normalized.includes("window.__tauri_internal") ||
    normalized.includes("window.__tauri")
  );
}

/**
 * Show native Save dialog via Tauri and persist blob to selected path.
 * Returns:
 * - "saved": user picked a path and save succeeded
 * - "cancelled": user cancelled dialog
 * - "unavailable": native API unavailable (fallback needed)
 */
export async function saveBlobWithDialog(
  filename: string,
  blob: Blob,
): Promise<SaveFileWithDialogResult> {
  try {
    const path = await save({ defaultPath: filename });
    if (!path || (Array.isArray(path) && path.length === 0)) {
      return "cancelled";
    }

    const targetPath = Array.isArray(path) ? path[0] : path;
    const bytes = new Uint8Array(await blob.arrayBuffer());
    await writeFile(targetPath, bytes);
    return "saved";
  } catch (error) {
    if (isPluginUnavailableError(error)) {
      return "unavailable";
    }
    throw error;
  }
}
