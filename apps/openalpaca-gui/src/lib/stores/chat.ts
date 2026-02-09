/**
 * Chat store — reactive state for the chat panel.
 */

import { writable, get } from "svelte/store";
import { events, type ServerEvent } from "../daemon";
import {
  sendMessage as apiSendMessage,
  getChatHistory,
  clearChatHistory as apiClearHistory,
  createChatStream,
} from "../api/chat";
import type { ChatMessage, ChatStreamDoneData } from "../types";

export const chatMessages = writable<ChatMessage[]>([]);
export const chatLoading = writable(false);
export const chatError = writable<string | null>(null);
export const chatStreaming = writable(false);
export const currentStreamId = writable<string | null>(null);
export const activeLaneKey = writable<string | null>(null);

let nextLocalId = -1;

/** Load conversation history from the API. */
export async function loadHistory(): Promise<void> {
  try {
    const resp = await getChatHistory(100, 0);
    chatMessages.set(resp.messages);
    activeLaneKey.set(resp.lane_key);
  } catch (e) {
    console.error("[chat-store] Failed to load history:", e);
  }
}

/** Send a message and connect to SSE for the response. */
export async function sendChatMessage(content: string): Promise<void> {
  chatLoading.set(true);
  chatError.set(null);

  // Optimistic user message
  const userMsg: ChatMessage = {
    id: nextLocalId--,
    lane_key: get(activeLaneKey) ?? "(pending)",
    role: "user",
    content,
    created_at: new Date().toISOString(),
  };
  chatMessages.update((msgs) => [...msgs, userMsg]);

  try {
    const resp = await apiSendMessage({ content });
    currentStreamId.set(resp.stream_id);
    chatStreaming.set(true);

    // Update lane key from server response and patch optimistic message
    activeLaneKey.set(resp.lane_key);
    chatMessages.update((msgs) =>
      msgs.map((m) => m.id === userMsg.id ? { ...m, lane_key: resp.lane_key } : m)
    );

    const es = createChatStream(resp.stream_id);
    if (!es) {
      chatError.set("Not connected to daemon");
      chatLoading.set(false);
      chatStreaming.set(false);
      return;
    }

    // Placeholder for assistant response while streaming
    let assistantMsgId = nextLocalId--;
    const thinkingMsg: ChatMessage = {
      id: assistantMsgId,
      lane_key: resp.lane_key,
      role: "assistant",
      content: "",
      created_at: new Date().toISOString(),
    };
    chatMessages.update((msgs) => [...msgs, thinkingMsg]);

    es.addEventListener("thinking", () => {
      chatMessages.update((msgs) =>
        msgs.map((m) =>
          m.id === assistantMsgId ? { ...m, content: "Thinking..." } : m,
        ),
      );
    });

    es.addEventListener("delta", (event: MessageEvent) => {
      try {
        const data = JSON.parse(event.data);
        chatMessages.update((msgs) =>
          msgs.map((m) =>
            m.id === assistantMsgId
              ? { ...m, content: m.content === "Thinking..." ? data.content : m.content + data.content }
              : m,
          ),
        );
      } catch {
        // ignore parse errors
      }
    });

    es.addEventListener("done", (event: MessageEvent) => {
      try {
        const data: ChatStreamDoneData = JSON.parse(event.data);
        chatMessages.update((msgs) =>
          msgs.map((m) =>
            m.id === assistantMsgId
              ? {
                  ...m,
                  content: data.content,
                  model: data.model,
                  tokens_in: data.tokens_in,
                  tokens_out: data.tokens_out,
                  duration_ms: data.duration_ms,
                }
              : m,
          ),
        );
      } catch {
        // ignore parse errors
      }
      es.close();
      chatStreaming.set(false);
      chatLoading.set(false);
      currentStreamId.set(null);
    });

    es.addEventListener("error", (event: MessageEvent | Event) => {
      let message = "Stream error";
      if ("data" in event && event.data) {
        try {
          const data = JSON.parse((event as MessageEvent).data);
          message = data.message || message;
        } catch {
          // ignore
        }
      }
      chatError.set(message);
      chatMessages.update((msgs) =>
        msgs.map((m) =>
          m.id === assistantMsgId ? { ...m, content: `Error: ${message}` } : m,
        ),
      );
      es.close();
      chatStreaming.set(false);
      chatLoading.set(false);
      currentStreamId.set(null);
    });

    es.onerror = () => {
      // Connection-level error (different from SSE "error" event)
      es.close();
      chatStreaming.set(false);
      chatLoading.set(false);
      currentStreamId.set(null);
    };
  } catch (e) {
    chatError.set(e instanceof Error ? e.message : String(e));
    chatLoading.set(false);
    chatStreaming.set(false);
  }
}

/** Clear conversation history. */
export async function clearHistory(): Promise<void> {
  try {
    await apiClearHistory();
    chatMessages.set([]);
    chatError.set(null);
  } catch (e) {
    console.error("[chat-store] Failed to clear history:", e);
    chatError.set(e instanceof Error ? e.message : String(e));
  }
}

/** Subscribe to WebSocket chat events for cross-tab awareness. */
export function subscribeToChatEvents(): () => void {
  return events.subscribe(($events) => {
    if ($events.length === 0) return;
    const latest = $events[0] as ServerEvent;
    if (latest.type === "chat_stream_started") {
      // Could trigger UI updates if needed
    } else if (latest.type === "chat_stream_ended") {
      // Could trigger history reload
    }
  });
}

/** Subscribe to task_status WebSocket events and inject completed/failed results as chat messages. */
export function subscribeToTaskResultEvents(): () => void {
  return events.subscribe(($events) => {
    if ($events.length === 0) return;
    const latest = $events[0] as ServerEvent;
    if (latest.type !== "task_status") return;
    if (latest.status !== "completed" && latest.status !== "failed") return;
    if (!latest.result_summary) return;

    const content = latest.status === "completed"
      ? `**Task completed: ${latest.title || "Task"}**\n\n${latest.result_summary}`
      : `**Task failed: ${latest.title || "Task"}**\n\n${latest.result_summary}`;

    chatMessages.update((msgs) => [
      ...msgs,
      {
        id: nextLocalId--,
        lane_key: get(activeLaneKey) ?? "unknown",
        role: "assistant" as const,
        content,
        created_at: latest.ts,
      },
    ]);
  });
}
