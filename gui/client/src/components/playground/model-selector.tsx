"use client";

import { useEffect, useState } from "react";
import { Cpu } from "lucide-react";
import { fetchNodeModels } from "@/lib/api/node";

interface ModelSelectorProps {
  value?: string;
  onChange?: (model: string) => void;
}

export function ModelSelector({ value = "llama3.2", onChange }: ModelSelectorProps) {
  const [models, setModels] = useState<string[]>([]);
  const [isLoading, setIsLoading] = useState(true);

  useEffect(() => {
    let isMounted = true;
    async function loadModels() {
      try {
        const fetched = await fetchNodeModels();
        if (isMounted && fetched.length > 0) {
          const modelIds = fetched.map((m) => m.id);
          setModels(modelIds);
          if (onChange && !modelIds.includes(value)) {
            onChange(modelIds[0]);
          }
        } else if (isMounted) {
          setModels(["llama3.2", "mistral", "qwen2"]);
        }
      } catch (e) {
        if (isMounted) {
          setModels(["llama3.2", "mistral", "qwen2"]);
        }
      } finally {
        if (isMounted) setIsLoading(false);
      }
    }
    loadModels();
    return () => {
      isMounted = false;
    };
  }, []);

  return (
    <div className="flex items-center gap-2">
      <Cpu className="w-4 h-4 text-muted-foreground" />
      <select
        value={value}
        onChange={(e) => onChange?.(e.target.value)}
        disabled={isLoading}
        className="h-8 rounded-md border border-border bg-card px-2 py-1 text-xs font-medium text-foreground focus:outline-none focus:ring-1 focus:ring-ring cursor-pointer"
      >
        {models.map((m) => (
          <option key={m} value={m}>
            {m}
          </option>
        ))}
      </select>
    </div>
  );
}
