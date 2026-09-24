/**
 * The transcript's "notes used in this answer", named.
 *
 * `done.memory_used` carries ids and revisions, not titles, so each note is
 * read once through the same query the Memory view keys it by — under this
 * window's project, which is what decides whether a project note is visible —
 * and a note that is gone since stays an id rather than a guess.
 */

import { useQueries } from "@tanstack/react-query";

import { MemoryUsedNote } from "@/components/chat";
import { getMemory } from "@/lib/api/memories";
import type { MemoryUsedRef } from "@/lib/chat-stream";
import { qk } from "@/lib/query-keys";
import { useProjectStore } from "@/stores/project";
import { useUiStore } from "@/stores/ui";

export function TranscriptMemoryUsed({
  items,
}: {
  items: readonly MemoryUsedRef[];
}) {
  const projectPath = useProjectStore((s) => s.path);
  const openMemory = useUiStore((s) => s.openMemory);
  const notes = useQueries({
    queries: items.map((item) => ({
      queryKey: qk.memory.detail(item.id, projectPath),
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        getMemory(item.id, projectPath, {}, signal),
    })),
  });

  return (
    <MemoryUsedNote
      items={items.map((item, index) => ({
        id: item.id,
        title: notes[index]?.data?.title ?? null,
        reason: item.reason,
      }))}
      onOpen={openMemory}
    />
  );
}
