/**
 * `USER.md` as React state — Settings → Persona (memory redesign, design §6
 * *USER.md*; MD-3, MD-9).
 *
 * No event names a `USER.md` write, so nothing but this window's own save
 * refreshes the query; a write from elsewhere — the onboarding
 * `update_persona`, `openalpaca persona edit user` — reaches the editor as a
 * `409` on save, which is the design's answer. A successful save puts what was
 * written, under the revision the daemon answered, straight into the cache:
 * the file is exactly those bytes now, and a refetch would only flash the old
 * text while it ran.
 */

import {
  useMutation,
  useQuery,
  useQueryClient,
  type UseMutationResult,
  type UseQueryResult,
} from "@tanstack/react-query";

import {
  getPersonaUser,
  putPersonaUser,
  type PersonaUser,
} from "@/lib/api/persona";
import { qk } from "@/lib/query-keys";

/** `GET /v1/persona/user` — the document and the revision it is. */
export function usePersonaUser(): UseQueryResult<PersonaUser> {
  return useQuery({
    queryKey: qk.persona.user(),
    queryFn: ({ signal }) => getPersonaUser(signal),
  });
}

export interface SavePersonaVariables {
  content: string;
  /** The revision the draft is an edit of — sent as `If-Match`. */
  etag: string;
}

/** `PUT /v1/persona/user`. */
export function useSavePersonaUser(): UseMutationResult<
  { etag: string },
  Error,
  SavePersonaVariables
> {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ content, etag }: SavePersonaVariables) =>
      putPersonaUser(content, etag),
    onSuccess: (written, { content }) => {
      client.setQueryData<PersonaUser>(qk.persona.user(), {
        content,
        etag: written.etag,
      });
    },
  });
}
