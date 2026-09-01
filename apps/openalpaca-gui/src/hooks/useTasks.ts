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
  listTasks,
  performTaskAction,
  type ListTasksQuery,
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

export interface TaskActionInput {
  id: string;
  action: TaskAction;
}

/** `POST /v1/tasks/{id}/action` — 409 carries a human message for the toast. */
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
