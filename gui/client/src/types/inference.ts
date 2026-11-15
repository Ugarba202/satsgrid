export interface Message {
  role: "system" | "user" | "assistant";
  content: string;
  receipt_id?: string;
}

export interface ChatCompletionRequest {
  model: string;
  messages: Message[];
  stream?: boolean;
}
