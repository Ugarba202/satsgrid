import { useEffect, useRef, useCallback } from "react";
import { useSpeechSynthesis } from "./useSpeechSynthesis";

export type AnnouncementEventType =
  | "invoice_issued"
  | "payment_confirmed"
  | "inference_completed"
  | "receipt_available"
  | "inference_failed"
  | "attention_required";

export interface SpokenAnnouncementConfig {
  enabled: boolean;
}

export interface UseSpokenAnnouncementsReturn {
  announce: (event: AnnouncementEventType, details?: string) => void;
  stopAnnouncements: () => void;
  isSpeaking: boolean;
}

export function formatAnnouncementMessage(
  event: AnnouncementEventType,
  details?: string
): string {
  switch (event) {
    case "invoice_issued":
      return "Invoice issued. Awaiting payment.";
    case "payment_confirmed":
      return "Payment confirmed. Inference authorized.";
    case "inference_completed":
      return "Inference completed. Your response is ready.";
    case "receipt_available":
      return "Signed inference receipt available.";
    case "inference_failed":
      return details ? `Inference failed. ${details}` : "The inference request failed. Check the displayed error.";
    case "attention_required":
      return details ? `Notice: ${details}` : "Session status requires attention.";
    default:
      return "";
  }
}

export function useSpokenAnnouncements(
  config: SpokenAnnouncementConfig
): UseSpokenAnnouncementsReturn {
  const { speak, stop, isSpeaking } = useSpeechSynthesis();
  const announcedEventsRef = useRef<Set<string>>(new Set());

  const announce = useCallback(
    (event: AnnouncementEventType, details?: string) => {
      if (!config.enabled) return;

      const eventKey = `${event}:${details || ""}`;
      // Prevent repeated immediate duplicate announcements for streaming events
      if (announcedEventsRef.current.has(eventKey)) {
        return;
      }

      announcedEventsRef.current.add(eventKey);
      // Expire deduplication after 5 seconds
      setTimeout(() => {
        announcedEventsRef.current.delete(eventKey);
      }, 5000);

      const message = formatAnnouncementMessage(event, details);
      if (message) {
        speak(message);
      }
    },
    [config.enabled, speak]
  );

  const stopAnnouncements = useCallback(() => {
    stop();
    announcedEventsRef.current.clear();
  }, [stop]);

  useEffect(() => {
    return () => {
      stopAnnouncements();
    };
  }, [stopAnnouncements]);

  return {
    announce,
    stopAnnouncements,
    isSpeaking,
  };
}
