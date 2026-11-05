import { ChatCompletionRequest, Message } from "@/types/inference";

export async function streamChatCompletion(
  request: ChatCompletionRequest,
  macaroon: string,
  preimage: string,
  onChunk: (text: string) => void,
  onChargedSats?: (sats: number) => void,
  onRemainingBudget?: (sats: number) => void
): Promise<void> {
  const res = await fetch("/v1/chat/completions", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "Authorization": `L402 ${macaroon}:${preimage}`
    },
    body: JSON.stringify({ ...request, stream: true }),
  });

  if (!res.ok) {
    throw new Error(`API Error: ${res.status}`);
  }

  const charged = res.headers.get("x-satsgrid-charged-sats");
  if (charged && onChargedSats) {
    onChargedSats(parseInt(charged, 10));
  }

  const remaining = res.headers.get("x-satsgrid-remaining-budget-sats");
  if (remaining && onRemainingBudget) {
    onRemainingBudget(parseInt(remaining, 10));
  }

  if (!res.body) throw new Error("No response body");

  const reader = res.body.getReader();
  const decoder = new TextDecoder("utf-8");
  let buffer = "";

  while (true) {
    const { done, value } = await reader.read();
    if (done) {
      if (buffer) {
        // process any remaining buffer if needed
      }
      break;
    }

    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split("\n");
    buffer = lines.pop() || "";

    for (const line of lines) {
      if (line.startsWith("data: ") && line !== "data: [DONE]" && line !== "data: [DONE]\r") {
        try {
          const data = JSON.parse(line.slice(6));
          if (data.choices && data.choices[0].delta && data.choices[0].delta.content) {
            onChunk(data.choices[0].delta.content);
          }
        } catch (e) {
          console.error("Failed to parse SSE chunk", e);
        }
      }
    }
  }
}
