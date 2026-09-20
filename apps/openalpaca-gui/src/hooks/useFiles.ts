/**
 * File metadata and host-side file actions.
 *
 * Content is read through `api/artifacts` instead: `useArtifactText` for
 * characters and `artifactContentUrl` for the URL an `<img>` loads. `download`
 * here is still the one that hands the viewer a blob to save.
 */

import {
  useMutation,
  useQuery,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";
import {
  downloadFile,
  getFileMetadata,
  openFileWithSystemDefault,
} from "@/lib/api/files";
import type { FileAsset, FileOpenResponse } from "@/lib/api/types";
import { qk } from "@/lib/query-keys";

export function useFileMetadata(id: string | null): UseQueryResult<FileAsset> {
  return useQuery({
    queryKey: qk.files.metadata(id ?? ""),
    queryFn: ({ signal }) => getFileMetadata(id as string, signal),
    enabled: id !== null,
  });
}

export function useDownloadFile(): UseMutationResult<Blob, Error, string> {
  return useMutation({ mutationFn: (id: string) => downloadFile(id) });
}

/** Opens with the daemon host's default app — this is not "Reveal in Finder". */
export function useOpenFile(): UseMutationResult<
  FileOpenResponse,
  Error,
  string
> {
  return useMutation({
    mutationFn: (id: string) => openFileWithSystemDefault(id),
  });
}
