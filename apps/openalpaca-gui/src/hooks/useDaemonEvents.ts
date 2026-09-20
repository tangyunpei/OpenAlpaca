/**
 * Live `ServerEvent` access.
 *
 * The socket is best-effort — the daemon drops frames for a lagged client with
 * no notification — so `useResyncSignal` exposes the "you may have missed
 * events" hint that `QueryProvider` already uses to invalidate the cache.
 */

import { useEffect, useRef, useState } from "react";

import {
  daemonEvents,
  type EventsStatus,
  type ResyncSignal,
  type ServerEvent,
  type ServerEventType,
} from "@/lib/events";

export function useEventsStatus(): EventsStatus {
  const [status, setStatus] = useState<EventsStatus>(() =>
    daemonEvents.getStatus(),
  );
  useEffect(() => daemonEvents.onStatus(setStatus), []);
  return status;
}

/**
 * Subscribe to a subset of the firehose. The handler is held in a ref, so an
 * inline arrow does not re-subscribe on every render.
 */
export function useServerEvent(
  types: readonly ServerEventType[],
  handler: (event: ServerEvent) => void,
): void {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  const key = types.join("|");
  useEffect(() => {
    const wanted = new Set<string>(key.split("|"));
    return daemonEvents.onEvent((event) => {
      if (wanted.has(event.type)) handlerRef.current(event);
    });
  }, [key]);
}

/** The retained ring, newest first. Backs the Event log surfaces. */
export function useEventRing(limit = 200): ServerEvent[] {
  const [events, setEvents] = useState<ServerEvent[]>(() => [
    ...daemonEvents.getEvents(),
  ]);
  useEffect(
    () => daemonEvents.onEvent(() => setEvents([...daemonEvents.getEvents()])),
    [],
  );
  return events.slice(0, limit);
}

/** The most recent "possibly missed events" signal, or `null`. */
export function useResyncSignal(): ResyncSignal | null {
  const [signal, setSignal] = useState<ResyncSignal | null>(null);
  useEffect(() => daemonEvents.onResync(setSignal), []);
  return signal;
}
