/**
 * Typed fetch wrapper for the daemon's `/v1/*` surface.
 *
 * Two error-envelope styles exist and they are inconsistent (API_MAP §5):
 *   chat/settings   → `{ error: { code, message } }` (settings adds `status`)
 *   tasks/agents/…  → `{ error: "<string>" }`, sometimes with `details`
 *   auth failures   → `401 "Invalid token"` as plain text
 * `parseErrorPayload` handles all of them so callers never have to.
 */

import { ensureConnection, httpUrl, type ConnectionInfo } from "./connection";

export type QueryValue = string | number | boolean | undefined | null;

export interface ApiRequestInit {
  method?: "GET" | "POST" | "PUT" | "DELETE" | "PATCH";
  /** JSON body. Mutually exclusive with `formData`. */
  body?: unknown;
  /** Multipart body (file uploads). Content-Type is left to the browser. */
  formData?: FormData;
  query?: Record<string, QueryValue>;
  signal?: AbortSignal;
  headers?: Record<string, string>;
}

/** A non-2xx response, or a transport failure, from the daemon. */
export class ApiError extends Error {
  override readonly name = "ApiError";

  constructor(
    message: string,
    readonly status: number,
    /** Envelope error code (`NOT_FOUND`, `STREAM_NOT_FOUND`, …) when present. */
    readonly code: string | null = null,
    /** The parsed body, for callers that need `cap`, `details`, etc. */
    readonly detail: unknown = null,
  ) {
    super(message);
  }

  /** `status === 0` means the request never reached the daemon. */
  get isTransport(): boolean {
    return this.status === 0;
  }

  get isNotFound(): boolean {
    return this.status === 404;
  }

  get isUnauthorized(): boolean {
    return this.status === 401 || this.status === 403;
  }

  /** 4xx (except 408/429) is a client mistake — retrying will not help. */
  get isRetryable(): boolean {
    if (this.isTransport) return true;
    if (this.status === 408 || this.status === 429) return true;
    return this.status >= 500;
  }
}

export interface ParsedApiError {
  code: string | null;
  message: string;
}

/**
 * Normalize any daemon error body into `{ code, message }`.
 *
 * Exported for tests and for callers that hold a body already (e.g. XHR
 * uploads, which do not go through `apiFetch`).
 */
export function parseErrorPayload(
  payload: unknown,
  fallback: string,
): ParsedApiError {
  if (typeof payload === "string") {
    const trimmed = payload.trim();
    return { code: null, message: trimmed.length > 0 ? trimmed : fallback };
  }

  if (typeof payload !== "object" || payload === null) {
    return { code: null, message: fallback };
  }

  const body = payload as Record<string, unknown>;
  const error = body.error;

  // `{ error: { code, message } }`
  if (typeof error === "object" && error !== null) {
    const envelope = error as Record<string, unknown>;
    const message =
      typeof envelope.message === "string" ? envelope.message : fallback;
    const code = typeof envelope.code === "string" ? envelope.code : null;
    return { code, message };
  }

  // `{ error: "string" }` — optionally with `details`
  if (typeof error === "string" && error.length > 0) {
    const details = typeof body.details === "string" ? body.details : null;
    return { code: null, message: details ? `${error}: ${details}` : error };
  }

  // A few handlers reply with a bare `{ message }`.
  if (typeof body.message === "string" && body.message.length > 0) {
    return { code: null, message: body.message };
  }

  return { code: null, message: fallback };
}

/** Read a response body as JSON, falling back to text, never throwing. */
async function readBody(response: Response): Promise<unknown> {
  const text = await response.text().catch(() => "");
  if (text.length === 0) return null;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return text;
  }
}

/** Build an `ApiError` from a non-2xx response. */
export async function errorFromResponse(response: Response): Promise<ApiError> {
  const payload = await readBody(response);
  const fallback =
    response.statusText || `Request failed with status ${response.status}`;
  const { code, message } = parseErrorPayload(payload, fallback);
  return new ApiError(message, response.status, code, payload);
}

export function buildQuery(
  query: Record<string, QueryValue> | undefined,
): string {
  if (!query) return "";
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined || value === null) continue;
    params.set(key, String(value));
  }
  const qs = params.toString();
  return qs.length > 0 ? `?${qs}` : "";
}

function buildHeaders(info: ConnectionInfo, init: ApiRequestInit): Headers {
  const headers = new Headers(init.headers);
  headers.set("Authorization", `Bearer ${info.token}`);
  if (init.body !== undefined && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }
  return headers;
}

/** Issue an authenticated request and return the raw `Response`. */
export async function apiRequest(
  path: string,
  init: ApiRequestInit = {},
): Promise<Response> {
  const info = await ensureConnection();
  const url = httpUrl(info, `${path}${buildQuery(init.query)}`);

  let response: Response;
  try {
    response = await fetch(url, {
      method: init.method ?? "GET",
      headers: buildHeaders(info, init),
      body:
        init.formData ??
        (init.body !== undefined ? JSON.stringify(init.body) : undefined),
      signal: init.signal,
    });
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === "AbortError")
      throw cause;
    const message =
      cause instanceof Error ? cause.message : "Network request failed";
    throw new ApiError(message, 0, null, cause);
  }

  if (!response.ok) throw await errorFromResponse(response);
  return response;
}

/**
 * Issue an authenticated request and decode a JSON body.
 *
 * Routes that answer `200 OK` with an empty body (confirmations, key mutations)
 * resolve to `undefined`; type those callers as `apiFetch<void>`.
 */
export async function apiFetch<T>(
  path: string,
  init: ApiRequestInit = {},
): Promise<T> {
  const response = await apiRequest(path, init);
  const text = await response.text();
  if (text.length === 0) return undefined as T;
  return JSON.parse(text) as T;
}

/** Issue an authenticated request and return the body as a `Blob`. */
export async function apiFetchBlob(
  path: string,
  init: ApiRequestInit = {},
): Promise<Blob> {
  const response = await apiRequest(path, init);
  return await response.blob();
}
