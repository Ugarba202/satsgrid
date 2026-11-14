import { ChatCompletionRequest } from "@/types/inference";

export class InferenceRequestError extends Error {
  constructor(message: string, readonly status: number) {
    super(message);
    this.name = "InferenceRequestError";
  }
}

async function responseError(res: Response, credentials: string[]): Promise<Error> {
  let message = res.statusText || "Request failed";
  try {
    const body = await res.json();
    const candidate = body?.error?.message ?? body?.error ?? body?.message;
    if (typeof candidate === "string" && candidate.trim()) message = candidate;
  } catch {
    // Keep the HTTP status when the backend response isn't JSON.
  }
  for (const credential of credentials) {
    if (credential) message = message.replaceAll(credential, "[redacted]");
  }
  return new InferenceRequestError(`Inference failed (HTTP ${res.status}): ${message}`, res.status);
}

function numericHeader(res: Response, name: string): number | undefined {
  const raw = res.headers.get(name);
  if (!raw) return undefined;
  const value = Number(raw);
  return Number.isFinite(value) && value >= 0 ? value : undefined;
}

export async function streamChatCompletion(
  request: ChatCompletionRequest,
  macaroon: string,
  preimage: string,
  onChunk: (text: string) => void,
  onChargedSats?: (sats: number) => void,
  onRemainingBudget?: (sats: number) => void,
  onAuthorized?: () => void,
  onCompleted?: () => void | Promise<void>
): Promise<void> {
  const res = await fetch("/v1/chat/completions", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `L402 ${macaroon}:${preimage}`,
    },
    body: JSON.stringify({ ...request, stream: true }),
  });

  if (!res.ok) throw await responseError(res, [macaroon, preimage]);

  // A successful response means the backend accepted the L402 proof and admitted
  // the request. The UI must not infer authorization from the wallet response.
  onAuthorized?.();
  const charged = numericHeader(res, "x-satsgrid-charged-sats");
  const remaining = numericHeader(res, "x-satsgrid-remaining-budget-sats");

  if (!res.body) throw new Error("Inference response has no stream body");
  const reader = res.body.getReader();
  const decoder = new TextDecoder("utf-8");
  let buffer = "";
  let eventData: string[] = [];
  let done = false;

  const dispatchEvent = () => {
    if (eventData.length === 0) return;
    const dataText = eventData.join("\n");
    eventData = [];
    if (dataText.trim() === "[DONE]") {
      done = true;
      return;
    }
    let data: {
      choices?: Array<{ delta?: { content?: unknown } }>;
      error?: { message?: unknown };
    };
    try {
      data = JSON.parse(dataText) as typeof data;
    } catch {
      return;
    }
    const content = data?.choices?.[0]?.delta?.content;
    if (typeof content === "string" && content.length > 0) onChunk(content);
    const errorMessage = data.error?.message;
    if (typeof errorMessage === "string" && errorMessage.trim()) throw new Error(errorMessage);
  };

  const consumeLines = (flush = false) => {
    const lines = buffer.split("\n");
    buffer = flush ? "" : (lines.pop() ?? "");
    if (flush && lines.at(-1) === "") lines.pop();
    for (const rawLine of lines) {
      const line = rawLine.endsWith("\r") ? rawLine.slice(0, -1) : rawLine;
      if (line === "") {
        dispatchEvent();
      } else if (line.startsWith("data:")) {
        eventData.push(line.slice(5).replace(/^ /, ""));
      }
    }
  };

  while (!done) {
    const { done: streamEnded, value } = await reader.read();
    if (streamEnded) {
      buffer += decoder.decode();
      consumeLines(true);
      dispatchEvent();
      break;
    }
    buffer += decoder.decode(value, { stream: true });
    consumeLines();
  }
  if (!done) throw new Error("Inference stream ended before successful completion ([DONE]).");
  if (charged !== undefined) onChargedSats?.(charged);
  if (remaining !== undefined) onRemainingBudget?.(remaining);
  await onCompleted?.();
  await reader.cancel().catch(() => undefined);
}

export async function chatCompletion(
  request: ChatCompletionRequest,
  macaroon: string,
  preimage: string,
  refreshSession: () => void | Promise<void>
): Promise<unknown> {
  const res = await fetch("/v1/chat/completions", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `L402 ${macaroon}:${preimage}`,
    },
    body: JSON.stringify({ ...request, stream: false }),
  });
  if (!res.ok) throw await responseError(res, [macaroon, preimage]);
  const result = await res.json();
  await refreshSession();
  return result;
}
