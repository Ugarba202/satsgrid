import Link from "next/link";
import { Button } from "@/components/ui/button";
import { Zap, MessageSquare, ShieldCheck, Cpu, Lock, Wallet, Activity, Server, User } from "lucide-react";
import { Navbar } from "@/components/layout/navbar";
import { Footer } from "@/components/layout/footer";

export default function Home() {
  return (
    <div className="flex min-h-screen flex-col bg-background selection:bg-primary/30 selection:text-primary-foreground">
      <Navbar />

      <main className="flex-1">
        {/* HERO SECTION */}
        <section className="relative overflow-hidden border-b border-border/40 py-24 md:py-32 lg:py-40">
          <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[600px] h-[600px] bg-primary/10 blur-[120px] rounded-full pointer-events-none" />
          
          <div className="container relative z-10 mx-auto max-w-6xl px-4 text-center sm:px-6">
            <h1 className="text-5xl sm:text-7xl font-bold tracking-tight text-foreground mb-6 uppercase">
              Open Models.<br />
              <span className="text-primary">Paid in Sats.</span>
            </h1>
            
            <p className="mx-auto max-w-2xl text-lg sm:text-xl text-muted-foreground leading-relaxed mb-10">
              Permissionless open-model inference powered by Lightning + cryptographic authorization.
            </p>
            
            <div className="flex flex-col sm:flex-row items-center justify-center gap-4">
              <Link href="/playground">
                <Button size="lg" className="h-14 px-8 text-lg font-medium w-full sm:w-auto">
                  Open Playground
                </Button>
              </Link>
              <Link href="/node">
                <Button size="lg" variant="outline" className="h-14 px-8 text-lg font-medium border-border hover:bg-card w-full sm:w-auto">
                  Run a Node
                </Button>
              </Link>
            </div>
          </div>
        </section>

        {/* HOW IT WORKS SECTION */}
        <section id="how-it-works" className="py-24 border-b border-border/40 bg-card/30">
          <div className="container mx-auto max-w-6xl px-4 sm:px-6 text-center">
            <h2 className="text-3xl font-bold tracking-tight sm:text-4xl mb-16 uppercase">How it works</h2>

            <div className="mx-auto flex max-w-3xl flex-col sm:flex-row items-center justify-between gap-6 font-mono text-lg font-semibold tracking-widest text-muted-foreground uppercase">
              <div className="flex flex-col items-center gap-4">
                <div className="bg-background border border-border p-4 rounded-full"><MessageSquare className="w-6 h-6 text-primary" /></div>
                <span>Request</span>
              </div>
              <span className="text-border hidden sm:block">→</span>
              <div className="flex flex-col items-center gap-4">
                <div className="bg-background border border-border p-4 rounded-full"><Wallet className="w-6 h-6 text-lightning" /></div>
                <span>Pay</span>
              </div>
              <span className="text-border hidden sm:block">→</span>
              <div className="flex flex-col items-center gap-4">
                <div className="bg-background border border-border p-4 rounded-full"><ShieldCheck className="w-6 h-6 text-success" /></div>
                <span>Prove</span>
              </div>
              <span className="text-border hidden sm:block">→</span>
              <div className="flex flex-col items-center gap-4">
                <div className="bg-background border border-border p-4 rounded-full"><Cpu className="w-6 h-6 text-foreground" /></div>
                <span>Infer</span>
              </div>
            </div>
          </div>
        </section>

        {/* FOR OPERATORS / CALLERS SECTION */}
        <section className="py-24 border-b border-border/40 bg-background">
          <div className="container mx-auto max-w-6xl px-4 sm:px-6">
            <h2 className="text-3xl font-bold tracking-tight sm:text-4xl text-center mb-16 uppercase">For Operators / Callers</h2>
            
            <div className="grid md:grid-cols-2 gap-8 max-w-4xl mx-auto">
              {/* Operator */}
              <div className="flex flex-col gap-6 rounded-2xl border border-border bg-card p-8">
                <div className="flex items-center gap-4 border-b border-border/50 pb-4">
                  <div className="p-3 rounded-lg bg-primary/20">
                    <Server className="w-6 h-6 text-primary" />
                  </div>
                  <h3 className="text-2xl font-bold">Operator</h3>
                </div>
                <ul className="space-y-4 font-mono text-sm tracking-wide text-muted-foreground">
                  <li className="flex items-center gap-3"><span className="text-success">✔</span> Run node</li>
                  <li className="flex items-center gap-3"><span className="text-success">✔</span> Set price</li>
                  <li className="flex items-center gap-3"><span className="text-success">✔</span> Earn sats</li>
                </ul>
              </div>

              {/* Caller */}
              <div className="flex flex-col gap-6 rounded-2xl border border-border bg-card p-8">
                <div className="flex items-center gap-4 border-b border-border/50 pb-4">
                  <div className="p-3 rounded-lg bg-foreground/10">
                    <User className="w-6 h-6 text-foreground" />
                  </div>
                  <h3 className="text-2xl font-bold">Caller</h3>
                </div>
                <ul className="space-y-4 font-mono text-sm tracking-wide text-muted-foreground">
                  <li className="flex items-center gap-3"><span className="text-success">✔</span> Request</li>
                  <li className="flex items-center gap-3"><span className="text-success">✔</span> Pay sats</li>
                  <li className="flex items-center gap-3"><span className="text-success">✔</span> Get inference</li>
                </ul>
              </div>
            </div>
          </div>
        </section>

        {/* FEATURES SECTION */}
        <section id="features" className="py-24 bg-card/30 border-b border-border/40">
          <div className="container mx-auto max-w-6xl px-4 sm:px-6">
            <h2 className="text-3xl font-bold tracking-tight sm:text-4xl text-center mb-16 uppercase">Built for Infrastructure</h2>

            <div className="grid gap-8 md:grid-cols-2 lg:grid-cols-3">
              <FeatureCard icon={Zap} title="Lightning-native" desc="Pay per inference instantly with satoshis over the Lightning Network." />
              <FeatureCard icon={Lock} title="Cryptographic Auth" desc="Secure L402 + macaroon caveats ensure verifiable authorization." />
              <FeatureCard icon={ShieldCheck} title="Capability-based" desc="Payment strictly unlocks specific inference capabilities, nothing else." />
              <FeatureCard icon={Wallet} title="Bounded Sessions" desc="Set a strict maximum spend budget to control your inference costs." />
              <FeatureCard icon={Activity} title="Privacy by Default" desc="No accounts, no emails, no tracking. Prove you paid, not who you are." />
              <FeatureCard icon={Cpu} title="Open Models" desc="Independent operators can effortlessly serve open-weight models." />
            </div>
          </div>
        </section>

        {/* TECHNOLOGY STRIP */}
        <section className="py-12 bg-background">
          <div className="container mx-auto text-center px-4">
            <p className="font-mono text-sm tracking-widest text-muted-foreground uppercase">
              Lightning <span className="mx-4 text-border">·</span> 
              Rust <span className="mx-4 text-border">·</span> 
              Axum <span className="mx-4 text-border">·</span> 
              Ollama
            </p>
          </div>
        </section>
      </main>

      <Footer />
    </div>
  );
}

function FeatureCard({ icon: Icon, title, desc }: { icon: any, title: string, desc: string }) {
  return (
    <div className="flex flex-col gap-3">
      <div className="flex h-12 w-12 items-center justify-center rounded-lg border border-border bg-card">
        <Icon className="h-6 w-6 text-primary" />
      </div>
      <h3 className="text-lg font-semibold">{title}</h3>
      <p className="text-muted-foreground leading-relaxed">{desc}</p>
    </div>
  );
}
