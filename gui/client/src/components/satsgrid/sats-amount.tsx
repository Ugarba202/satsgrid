import { cn } from "@/lib/utils";

interface SatsAmountProps extends React.HTMLAttributes<HTMLDivElement> {
  amount: number;
  label?: string;
  showSuffix?: boolean;
}

export function SatsAmount({ amount, label, showSuffix = true, className, ...props }: SatsAmountProps) {
  return (
    <div className={cn("flex flex-col", className)} {...props}>
      {label && <span className="text-xs text-muted-foreground uppercase tracking-wider mb-1">{label}</span>}
      <div className="flex items-baseline gap-1">
        <span className="font-mono text-xl font-semibold text-foreground tracking-tight">
          {amount.toLocaleString()}
        </span>
        {showSuffix && <span className="text-sm font-medium text-muted-foreground">sats</span>}
      </div>
    </div>
  );
}
