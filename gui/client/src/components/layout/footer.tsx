import Link from "next/link";
import { Zap } from "lucide-react";

export function Footer() {
  return (
    <footer className="border-t border-border/40 bg-background py-12 md:py-16">
      <div className="container mx-auto max-w-6xl px-4 sm:px-6 flex flex-col md:flex-row justify-between items-center gap-6">
        <div className="flex items-center gap-2">
          <Zap className="h-5 w-5 text-primary" />
          <span className="font-semibold tracking-tight text-lg">SATSGRID</span>
        </div>
        
        <p className="text-sm text-muted-foreground">
          Built for the permissionless inference economy.
        </p>
        
        <div className="flex gap-4 text-sm font-medium text-muted-foreground">
          <Link href="/node" className="hover:text-foreground transition-colors">Run a Node</Link>
          <Link href="/playground" className="hover:text-foreground transition-colors">Playground</Link>
          <Link href="https://github.com/infernos-ai/infernos" target="_blank" className="hover:text-foreground transition-colors">Source Code</Link>
        </div>
      </div>
    </footer>
  );
}
