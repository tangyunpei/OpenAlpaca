/** Runs: list, detail, and the three legal actions. */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import {
  getTask,
  getTaskTimeline,
  listTasks,
  performTaskAction,
  rerunTask,
  type ListTasksQuery,
  type RerunResult,
  type TaskTimeline,
} from "@/lib/api/tasks";
import type {
  Task,
  TaskAction,
  TaskActionResponse,
  TaskDetailResponse,
} from "@/lib/api/types";
import { qk } from "@/lib/query-keys";

/** `GET /v1/tasks`. `status: "active"` is the daemon's special list mode. */
export function useTasks(query: ListTasksQuery = {}): UseQueryResult<Task[]> {
  return useQuery({
    queryKey: qk.tasks.list(query),
    queryFn: ({ signal }) => listTasks(query, signal),
    // Live status arrives over the WS; this is the reconciliation floor.
    staleTime: 10_000,
  });
}

export function useTask(id: string | null): UseQueryResult<TaskDetailResponse> {
  return useQuery({
    queryKey: qk.tasks.detail(id ?? ""),
    queryFn: ({ signal }) => getTask(id as string, signal),
    enabled: id !== null,
  });
}

/**
 * `GET /v1/tasks/{id}/timeline` — the Parallel work swimlanes.
 *
 * A live run's lanes move on every spawn and every completion, which arrive as
 * `subagent_span` frames; those invalidate `qk.tasks.timeline(id)`
 * (`lib/query-client.ts`), so this needs no polling. `blocked` is derived
 * server-side from what is pending *right now*, so a stale cache would show a
 * lane waiting on a prompt that was already answered — hence the short
 * `staleTime`, which is the floor for the refetch a `tool_confirmation`
 * resolution triggers by other means.
 */
export function useTaskTimeline(
  id: string | null,
): UseQueryResult<TaskTimeline> {
  return useQuery({
    queryKey: qk.tasks.timeline(id ?? ""),
    queryFn: ({ signal }) => getTaskTimeline(id as string, signal),
    enabled: id !== null && id !== "",
    staleTime: 5_000,
  });
}

export interface TaskActionInput {
  id: string;
  action: TaskAction;
}

/**
 * `POST /v1/tasks/{id}/action` — 409 carries a human message for the toast.
 *
 * Also the path for D5's `start`: same route, same response shape, same
 * invalidation, because the run it dispatches keeps the id that was asked
 * about. Only its refusals differ (`lib/api/tasks.ts` renders them).
 */
export function useTaskAction(): UseMutationResult<
  TaskActionResponse,
  Error,
  TaskActionInput
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (input: TaskActionInput) =>
      performTaskAction(input.id, input.action),
    onSuccess: (_data, input) => {
      void client.invalidateQueries({ queryKey: qk.tasks.all() });
      void client.invalidateQueries({ queryKey: qk.tasks.detail(input.id) });
    },
  });
}

/**
 * `POST /v1/tasks/{id}/rerun` — a **new** run from a finished one's goal.
 *
 * Both rows are invalidated: the list gains the copy, and the original's
 * detail is refetched because a re-run is the sort of thing a reader wants to
 * see reflected on the run they launched it from.
 */
export function useRerunTask(): UseMutationResult<RerunResult, Error, string> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => rerunTask(id),
    onSuccess: (result, id) => {
      void client.invalidateQueries({ queryKey: qk.tasks.all() });
      void client.invalidateQueries({ queryKey: qk.tasks.detail(id) });
      void client.invalidateQueries({
        queryKey: qk.tasks.detail(result.task_id),
      });
    },
  });
}
