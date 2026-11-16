import { useState, useEffect, useRef, useCallback } from "react";

export type SpeechRecognitionState = "idle" | "listening" | "stopped" | "error" | "unsupported";

export interface UseSpeechRecognitionOptions {
  onTranscript?: (transcript: string) => void;
  onError?: (error: string) => void;
}

export interface UseSpeechRecognitionReturn {
  isSupported: boolean;
  state: SpeechRecognitionState;
  transcript: string;
  error: string | null;
  startListening: () => void;
  stopListening: () => void;
  resetTranscript: () => void;
}

// Minimal Web Speech API typing for cross-browser support
interface SpeechRecognitionResultItem {
  transcript: string;
  confidence: number;
}

interface SpeechRecognitionResultList {
  readonly length: number;
  [index: number]: {
    readonly length: number;
    [index: number]: SpeechRecognitionResultItem;
    isFinal?: boolean;
  };
}

interface SpeechRecognitionEvent {
  resultIndex: number;
  results: SpeechRecognitionResultList;
}

interface SpeechRecognitionErrorEvent {
  error: string;
  message?: string;
}

interface SpeechRecognitionInstance {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  start: () => void;
  stop: () => void;
  abort: () => void;
  onstart: (() => void) | null;
  onend: (() => void) | null;
  onresult: ((event: SpeechRecognitionEvent) => void) | null;
  onerror: ((event: SpeechRecognitionErrorEvent) => void) | null;
}

type SpeechRecognitionConstructor = new () => SpeechRecognitionInstance;

declare global {
  interface Window {
    SpeechRecognition?: SpeechRecognitionConstructor;
    webkitSpeechRecognition?: SpeechRecognitionConstructor;
  }
}

export function useSpeechRecognition(
  options: UseSpeechRecognitionOptions = {}
): UseSpeechRecognitionReturn {
  const [isSupported, setIsSupported] = useState(false);
  const [state, setState] = useState<SpeechRecognitionState>("idle");
  const [transcript, setTranscript] = useState("");
  const [error, setError] = useState<string | null>(null);

  const recognitionRef = useRef<SpeechRecognitionInstance | null>(null);
  const optionsRef = useRef(options);

  useEffect(() => {
    optionsRef.current = options;
  });

  useEffect(() => {
    if (typeof window === "undefined") return;

    const SpeechRec = window.SpeechRecognition || window.webkitSpeechRecognition;
    if (SpeechRec) {
      setIsSupported(true);
      try {
        const recognition = new SpeechRec();
        recognition.continuous = true;
        recognition.interimResults = true;
        recognition.lang = "en-US";

        recognition.onstart = () => {
          setState("listening");
          setError(null);
        };

        recognition.onend = () => {
          // Keep stopped or idle state
          setState((prev) => (prev === "listening" ? "stopped" : prev));
        };

        recognition.onresult = (event: SpeechRecognitionEvent) => {
          let finalTranscript = "";
          let interimTranscript = "";
          for (let i = 0; i < event.results.length; i++) {
            const result = event.results[i];
            if (result && result[0]) {
              if (result.isFinal) {
                finalTranscript += result[0].transcript;
              } else {
                interimTranscript += result[0].transcript;
              }
            }
          }

          const combined = (finalTranscript + " " + interimTranscript).trim().replace(/\s+/g, " ");
          if (combined) {
            setTranscript(combined);
            optionsRef.current.onTranscript?.(combined);
          }
        };

        recognition.onerror = (event: SpeechRecognitionErrorEvent) => {
          let errorMsg = "Speech recognition error";
          if (event.error === "not-allowed" || event.error === "service-not-allowed") {
            errorMsg = "Microphone permission denied. Please allow microphone access in your browser.";
          } else if (event.error === "no-speech") {
            errorMsg = "No speech detected.";
          } else if (event.error === "network") {
            errorMsg = "Network error during speech recognition.";
          } else if (event.error === "audio-capture") {
            errorMsg = "No microphone was found or microphone is busy.";
          } else {
            errorMsg = `Speech recognition error: ${event.error}`;
          }

          setError(errorMsg);
          setState("error");
          optionsRef.current.onError?.(errorMsg);
        };

        recognitionRef.current = recognition;
      } catch {
        // Recognition constructor failed in browser environment
      }
    }

    return () => {
      if (recognitionRef.current) {
        try {
          recognitionRef.current.abort();
        } catch {
          // ignore cleanup abort errors
        }
        recognitionRef.current = null;
      }
    };
  }, []);

  const startListening = useCallback(() => {
    if (!isSupported || !recognitionRef.current) {
      setState("unsupported");
      setError("Speech recognition is not supported in this browser.");
      return;
    }

    try {
      setError(null);
      setTranscript("");
      recognitionRef.current.start();
    } catch {
      // If already started or failed to start
      try {
        recognitionRef.current.stop();
        setTimeout(() => {
          try {
            recognitionRef.current?.start();
          } catch {
            // ignore
          }
        }, 100);
      } catch {
        // ignore
      }
    }
  }, [isSupported]);

  const stopListening = useCallback(() => {
    if (recognitionRef.current) {
      try {
        recognitionRef.current.stop();
      } catch {
        // ignore
      }
    }
    setState("stopped");
  }, []);

  const resetTranscript = useCallback(() => {
    setTranscript("");
    setError(null);
    setState("idle");
  }, []);

  return {
    isSupported,
    state,
    transcript,
    error,
    startListening,
    stopListening,
    resetTranscript,
  };
}
