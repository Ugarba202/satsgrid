import { useState, useEffect, useRef, useCallback } from "react";

export interface UseSpeechSynthesisOptions {
  onStart?: () => void;
  onEnd?: () => void;
  onError?: (error: string) => void;
}

export interface UseSpeechSynthesisReturn {
  isSupported: boolean;
  isSpeaking: boolean;
  isPaused: boolean;
  error: string | null;
  speak: (text: string) => void;
  stop: () => void;
  pause: () => void;
  resume: () => void;
}

// Credential redactor: ensure no sensitive keys, macaroons, preimages, or URLs are read aloud
export function sanitizeSpeechText(text: string): string {
  if (!text) return "";

  let cleaned = text;

  // Redact 64-char hex strings (preimages / hashes)
  cleaned = cleaned.replace(/\b[0-9a-fA-F]{64}\b/g, "[redacted credential]");

  // Redact base64 macaroons / tokens
  cleaned = cleaned.replace(/AgE[A-Za-z0-9+/=_-]{30,}/g, "[redacted token]");

  // Redact NWC URIs
  cleaned = cleaned.replace(/nostr\+walletconnect:\/\/[^\s]+/gi, "[redacted wallet connection]");

  // Redact lightning invoices
  cleaned = cleaned.replace(/lnbc[a-z0-9]+/gi, "[lightning invoice]");

  // Redact internal keys or auth headers: e.g. "Authorization: L402 token:secret" or "Authorization L402 token:secret"
  cleaned = cleaned.replace(/(Authorization:?\s+)(L402|Bearer)?\s*[^\s]+/gi, "$1[redacted]");
  cleaned = cleaned.replace(/\bL402\s+[^\s]+/gi, "L402 [redacted]");

  return cleaned.trim();
}

// Split long text safely at sentence or clause boundaries to prevent browser speech timeout issues
export function chunkSpeechText(text: string, maxChunkLength = 180): string[] {
  const sanitized = sanitizeSpeechText(text);
  if (!sanitized) return [];

  // Match sentence terminators (. ! ? \n) followed by space or end of string
  const sentences = sanitized.match(/[^.!?\n]+[.!?\n]+|[^.!?\n]+$/g) || [sanitized];

  const chunks: string[] = [];
  let currentChunk = "";

  for (const sentence of sentences) {
    const trimmed = sentence.trim();
    if (!trimmed) continue;

    if ((currentChunk + " " + trimmed).trim().length <= maxChunkLength) {
      currentChunk = currentChunk ? `${currentChunk} ${trimmed}` : trimmed;
    } else {
      if (currentChunk) chunks.push(currentChunk);

      // If a single sentence is longer than maxChunkLength, split by commas or words
      if (trimmed.length > maxChunkLength) {
        const words = trimmed.split(/\s+/);
        let wordChunk = "";
        for (const word of words) {
          if ((wordChunk + " " + word).trim().length <= maxChunkLength) {
            wordChunk = wordChunk ? `${wordChunk} ${word}` : word;
          } else {
            if (wordChunk) chunks.push(wordChunk);
            wordChunk = word;
          }
        }
        if (wordChunk) currentChunk = wordChunk;
      } else {
        currentChunk = trimmed;
      }
    }
  }

  if (currentChunk) chunks.push(currentChunk);

  return chunks;
}

export function useSpeechSynthesis(
  options: UseSpeechSynthesisOptions = {}
): UseSpeechSynthesisReturn {
  const [isSupported] = useState(() => {
    if (typeof window === "undefined") return false;
    return "speechSynthesis" in window;
  });
  const [isSpeaking, setIsSpeaking] = useState(false);
  const [isPaused, setIsPaused] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const chunksRef = useRef<string[]>([]);
  const currentChunkIndexRef = useRef(0);
  const activeUtteranceRef = useRef<SpeechSynthesisUtterance | null>(null);
  const isCanceledRef = useRef(false);

  const optionsRef = useRef(options);
  useEffect(() => {
    optionsRef.current = options;
  });

  useEffect(() => {
    return () => {
      // Safe cleanup on unmount
      if (typeof window !== "undefined" && "speechSynthesis" in window) {
        try {
          window.speechSynthesis.cancel();
        } catch {
          // ignore
        }
      }
    };
  }, []);

  const speakNextChunkRef = useRef<() => void>(() => {});

  const speakNextChunk = useCallback(() => {
    if (isCanceledRef.current || !isSupported || typeof window === "undefined") {
      setIsSpeaking(false);
      setIsPaused(false);
      return;
    }

    if (currentChunkIndexRef.current >= chunksRef.current.length) {
      setIsSpeaking(false);
      setIsPaused(false);
      optionsRef.current.onEnd?.();
      return;
    }

    const chunkText = chunksRef.current[currentChunkIndexRef.current];
    try {
      const utterance = new SpeechSynthesisUtterance(chunkText);
      utterance.lang = "en-US";
      utterance.rate = 1.0;

      utterance.onstart = () => {
        setIsSpeaking(true);
        setIsPaused(false);
        if (currentChunkIndexRef.current === 0) {
          optionsRef.current.onStart?.();
        }
      };

      utterance.onend = () => {
        if (!isCanceledRef.current) {
          currentChunkIndexRef.current++;
          speakNextChunkRef.current();
        }
      };

      utterance.onerror = (e) => {
        if (e.error !== "canceled" && e.error !== "interrupted") {
          const err = `Speech synthesis error: ${e.error}`;
          setError(err);
          optionsRef.current.onError?.(err);
        }
        setIsSpeaking(false);
        setIsPaused(false);
      };

      activeUtteranceRef.current = utterance;
      window.speechSynthesis.speak(utterance);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Speech synthesis failed";
      setError(msg);
      setIsSpeaking(false);
      setIsPaused(false);
      optionsRef.current.onError?.(msg);
    }
  }, [isSupported]);

  useEffect(() => {
    speakNextChunkRef.current = speakNextChunk;
  }, [speakNextChunk]);

  const speak = useCallback(
    (rawText: string) => {
      if (!isSupported || typeof window === "undefined") {
        setError("Speech synthesis is not supported in this browser.");
        return;
      }

      // 1. Cancel any active speech to avoid overlapping utterances
      try {
        window.speechSynthesis.cancel();
      } catch {
        // ignore
      }

      setError(null);
      isCanceledRef.current = false;

      // 2. Chunk text safely
      const chunks = chunkSpeechText(rawText);
      if (chunks.length === 0) {
        setIsSpeaking(false);
        return;
      }

      chunksRef.current = chunks;
      currentChunkIndexRef.current = 0;

      // 3. Begin playback
      speakNextChunk();
    },
    [isSupported, speakNextChunk]
  );

  const stop = useCallback(() => {
    isCanceledRef.current = true;
    chunksRef.current = [];
    currentChunkIndexRef.current = 0;
    if (typeof window !== "undefined" && "speechSynthesis" in window) {
      try {
        window.speechSynthesis.cancel();
      } catch {
        // ignore
      }
    }
    setIsSpeaking(false);
    setIsPaused(false);
  }, []);

  const pause = useCallback(() => {
    if (typeof window !== "undefined" && "speechSynthesis" in window && isSpeaking && !isPaused) {
      try {
        window.speechSynthesis.pause();
        setIsPaused(true);
      } catch {
        // ignore
      }
    }
  }, [isSpeaking, isPaused]);

  const resume = useCallback(() => {
    if (typeof window !== "undefined" && "speechSynthesis" in window && isSpeaking && isPaused) {
      try {
        window.speechSynthesis.resume();
        setIsPaused(false);
      } catch {
        // ignore
      }
    }
  }, [isSpeaking, isPaused]);

  return {
    isSupported,
    isSpeaking,
    isPaused,
    error,
    speak,
    stop,
    pause,
    resume,
  };
}
