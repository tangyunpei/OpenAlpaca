/**
 * `QueryClientProvider` plus the live-event bridge.
 *
 * Mounting this is what connects the daemon socket: events invalidate the
 * cache, and a resync (reconnect or daemon restart) invalidates everything,
 * because the server drops frames for a lagged client without telling it.
 */

import { QueryClientProvider } from "@tanstack/react-query";
import { useEffect, type ReactNode } from "react";
import type { QueryClient } from "@tanstack/react-query";

import { daemonEvents } from "./events";
import {
  invalidateAfterResync,
  invalidateForEvent,
  queryClient,
} from "./query-client";

interface QueryProviderProps {
  children: ReactNode;
  /** Overridden in tests. */
  client?: QueryClient;
  /** Set to `false` to mount the cache without opening a socket. */
  connectEvents?: boolean;
}

export function QueryProvider({
  children,
  client = queryClient,
  connectEvents = true,
}: QueryProviderProps) {
  useEffect(() => {
    if (!connectEvents) return;

    const offEvent = daemonEvents.onEvent((event) => {
      invalidateForEvent(client, event);
    });
    const offResync = daemonEvents.onResync(() => {
      invalidateAfterResync(client);
    });

    void daemonEvents.connect();

    return () => {
      offEvent();
      offResync();
      daemonEvents.disconnect();
    };
  }, [client, connectEvents]);

  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
