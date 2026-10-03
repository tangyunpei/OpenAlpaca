/**
 * File metadata and host-side file actions.
 *
 * Content is read through `api/artifacts` instead: `useArtifactText` for
 * characters and `artifactContentUrl` for the URL an `<img>` loads. `download`
 * here is still the one that hands the viewer a blob to save; `Open` and
 * `Reveal in Finder` are `useArtifactFileActions`.
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
import { revealInFolder } from "@/lib/reveal";
import { useUiStore } from "@/stores/ui";

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

/** Opens with the daemon host's default app — `Open`, not `Reveal in Finder`. */
export function useOpenFile(): UseMutationResult<
  FileOpenResponse,
  Error,
  string
> {
  return useMutation({
    mutationFn: (id: string) => openFileWithSystemDefault(id),
  });
}

/**
 * `Open` and `Reveal in Finder` for one artifact, each answered by a toast —
 * the Library header, the `FILE WRITTEN` card and the file panel say the same
 * words.
 *
 * `open` is the daemon's route, so the file opens in its default app on the
 * daemon's host. `reveal` is the Tauri shell's `reveal_path` (`lib/reveal.ts`):
 * Finder comes forward with the file selected, so success needs no toast, and
 * a refusal is the shell's own sentence, shown as it is.
 */
export function useArtifactFileActions(): {
  open: (artifact: { id: string; name: string }) => void;
  reveal: (path: string) => void;
} {
  const openFile = useOpenFile();
  const showToast = useUiStore((s) => s.showToast);
  return {
    open: ({ id, name }) => {
      openFile.mutate(id, {
        onSuccess: () => showToast(`${name} opened in its default app`),
        onError: (error) => showToast(`Could not open — ${error.message}`),
      });
    },
    reveal: (path) => {
      revealInFolder(path).catch((error: unknown) =>
        showToast(error instanceof Error ? error.message : String(error)),
      );
    },
  };
}
