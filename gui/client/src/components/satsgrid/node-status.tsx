import { cn } from "@/lib/utils";
import { Server } from "lucide-react";

interface NodeStatusProps extends React.HTMLAttributes<HTMLDivElement> {
  isOnline: boolean;
  address?: string;
}

export function NodeStatus({ isOnline, address, className, ...props }: NodeStatusProps) {
  return (
    <div className={cn("flex items-center gap-3 bg-card border border-border rounded-lg px-3 py-2", className)} {...props}>
      <div className="p-1.5 rounded-md bg-secondary/50">
        <Server className="w-4 h-4 text-muted-foreground" />
      </div>
      <div className="flex flex-col">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium leading-none tracking-tight">SatsGrid Node</span>
          <span className="relative flex h-2 w-2">
            {isOnline && (
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-success opacity-75"></span>
            )}
            <span className={cn("relative inline-flex rounded-full h-2 w-2", isOnline ? "bg-success" : "bg-error")}></span>
          </span>
        </div>
        {address && (
          <span className="text-xs font-mono text-muted-foreground mt-1">
            {address}
          </span>
        )}
      </div>
    </div>
  );
}
