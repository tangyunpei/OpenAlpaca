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
import { uploadFile } from "../api/files";
import type { ChatMessage, ChatStreamDoneData, AttachmentRef, AttachmentDisplay } from "../types";

export const chatMessages = writable<ChatMessage[]>([]);
export const chatLoading = writable(false);
export const chatError = writable<string | null>(null);
export const chatStreaming = writable(false);
export const currentStreamId = writable<string | null>(null);
export const activeLaneKey = writable<string | null>(null);

/** Files selected by the user but not yet sent. */
export interface PendingFile {
  file: File;
  progress: number;  // 0-100
  error: string | null;
}

export const pendingFiles = writable<PendingFile[]>([]);
export const uploadingFiles = writable(false);

let nextLocalId = -1;

/** Dedupe key set: prevents duplicate task completion chat injections.
 *  Key format: "{task_id}:{terminal_status}" */
const injectedCompletions = new Set<string>();

type StructuredMessagePart = {
  type: string;
  file_id?: string;
  filename?: string;
  mime_type?: string;
};

function normalizeHistoryMessage(message: ChatMessage): ChatMessage {
  const normalized: ChatMessage = { ...message };
  if (
    normalized.content.trim().length === 0 &&
    normalized.display_text &&
    normalized.display_text.trim().length > 0
  ) {
    normalized.content = normalized.display_text;
  }

  if (
    (!normalized.attachments || normalized.attachments.length === 0) &&
    normalized.content_json
  ) {
    try {
      const parsed = JSON.parse(normalized.content_json) as { parts?: StructuredMessagePart[] };
      const parts = Array.isArray(parsed.parts) ? parsed.parts : [];
      const seen = new Set<string>();
      const attachments: AttachmentDisplay[] = [];
      for (const part of parts) {
        if (
          (part.type === "file_ref" || part.type === "document") &&
          part.file_id &&
          part.filename &&
          part.mime_type &&
          !seen.has(part.file_id)
        ) {
          seen.add(part.file_id);
          attachments.push({
            file_id: part.file_id,
            filename: part.filename,
            mime_type: part.mime_type,
            size_bytes: 0,
          });
        }
      }
      if (attachments.length > 0) {
        normalized.attachments = attachments;
      }
    } catch {
      // ignore malformed content_json
    }
  }

  if (
    normalized.content.trim().length === 0 &&
    normalized.attachments &&
    normalized.attachments.length > 0
  ) {
    normalized.content = "[Attachment]";
  }

  return normalized;
}

export function applyDoneDataToMessage(message: ChatMessage, data: ChatStreamDoneData): ChatMessage {
  return {
    ...message,
    content: data.content,
    model: data.model,
    tokens_in: data.tokens_in,
    tokens_out: data.tokens_out,
    duration_ms: data.duration_ms,
    citations: data.citations,
    artifacts: data.artifacts,
  };
}

/** Load conversation history from the API. */
export async function loadHistory(): Promise<void> {
  try {
    const resp = await getChatHistory(100, 0);
    chatMessages.set(resp.messages.map(normalizeHistoryMessage));
    activeLaneKey.set(resp.lane_key);
  } catch (e) {
    console.error("[chat-store] Failed to load history:", e);
  }
}

/** Add files to the pending list. */
export function addFiles(files: FileList | File[]): void {
  const newFiles = Array.from(files).map((file) => ({
    file,
    progress: 0,
    error: null,
  }));
  pendingFiles.update((existing) => [...existing, ...newFiles]);
}

/** Remove a pending file by index. */
export function removePendingFile(index: number): void {
  pendingFiles.update((files) => files.filter((_, i) => i !== index));
}

/** Clear all pending files. */
export function clearPendingFiles(): void {
  pendingFiles.set([]);
}

/** Send a message with optional file attachments and connect to SSE. */
export async function sendChatMessage(content: string): Promise<void> {
  chatLoading.set(true);
  chatError.set(null);

  const filesToUpload = get(pendingFiles);
  let attachments: AttachmentRef[] = [];
  let attachmentDisplays: AttachmentDisplay[] = [];

  // Upload pending files first
  if (filesToUpload.length > 0) {
    uploadingFiles.set(true);
    try {
      const results = await Promise.all(
        filesToUpload.map(async (pf, idx) => {
          const resp = await uploadFile(pf.file, (loaded, total) => {
            const pct = Math.round((loaded / total) * 100);
            pendingFiles.update((files) =>
              files.map((f, i) => (i === idx ? { ...f, progress: pct } : f)),
            );
          });
          return resp;
        }),
      );

      attachments = results.map((r) => ({ file_id: r.id }));
      attachmentDisplays = results.map((r) => ({
        file_id: r.id,
        filename: r.filename,
        mime_type: r.mime_type,
        size_bytes: r.size_bytes,
      }));
    } catch (e) {
      chatError.set(e instanceof Error ? e.message : "File upload failed");
      chatLoading.set(false);
      uploadingFiles.set(false);
      return;
    }
    uploadingFiles.set(false);
    clearPendingFiles();
  }

  // Optimistic user message
  const userMsg: ChatMessage = {
    id: nextLocalId--,
    lane_key: get(activeLaneKey) ?? "(pending)",
    role: "user",
    content:
      content.trim().length > 0
        ? content
        : attachmentDisplays.length > 0
          ? "[Attachment]"
          : content,
    created_at: new Date().toISOString(),
    attachments: attachmentDisplays.length > 0 ? attachmentDisplays : undefined,
  };
  chatMessages.update((msgs) => [...msgs, userMsg]);

  try {
    const req: { content: string; attachments?: AttachmentRef[] } = { content };
    if (attachments.length > 0) {
      req.attachments = attachments;
    }
    const resp = await apiSendMessage(req);
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
              ? applyDoneDataToMessage(m, data)
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
    resetCompletionDedupe();
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

    // Dedupe: skip if we already injected for this task + terminal status
    const dedupeKey = `${latest.task_id}:${latest.status}`;
    if (injectedCompletions.has(dedupeKey)) return;
    injectedCompletions.add(dedupeKey);

    // Build outcome-aware message content
    let content: string;
    const title = latest.title || "Task";

    if (latest.status === "completed") {
      const outcomeKind = latest.outcome_kind;
      const artifactCount = latest.artifact_count ?? 0;
      const summary = latest.outcome_summary || latest.result_summary || "Done";

      let outcomeLabel = "";
      if (outcomeKind === "text_only") {
        outcomeLabel = " (text only, no files)";
      } else if (outcomeKind === "artifact_only") {
        outcomeLabel = ` (${artifactCount} file${artifactCount !== 1 ? "s" : ""} produced)`;
      } else if (outcomeKind === "mixed") {
        outcomeLabel = ` (${artifactCount} file${artifactCount !== 1 ? "s" : ""} + text)`;
      }

      content = `**Task completed: ${title}**${outcomeLabel}\n\n${summary}`;
    } else {
      // failed
      const summary = latest.result_summary || "Unknown error";
      content = `**Task failed: ${title}**\n\n${summary}`;
    }

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

/** Reset completion injection dedupe (call on clearHistory). */
export function resetCompletionDedupe(): void {
  injectedCompletions.clear();
}
