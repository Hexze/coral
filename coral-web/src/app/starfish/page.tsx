import Link from "next/link";
import type { Metadata } from "next";
import { StarfishNav, StarfishFooter } from "@/components/starfish/Layout";

export const metadata: Metadata = {
  title: "Starfish",
  description: "Hypixel proxy with game overlay and Lua scripting.",
};

export default function StarfishPage() {
  return (
    <div className="min-h-screen flex flex-col">
      <StarfishNav active="starfish" />

      <section className="flex items-center justify-center pt-14 min-h-[60vh]">
        <div className="w-full max-w-xl mx-auto px-6 text-center py-20">
          <img src="/starfish.png" alt="Starfish" width={56} height={56} className="mx-auto mb-5 pixelated" />
          <h1 className="text-4xl sm:text-5xl font-bold tracking-tight mb-3">Starfish</h1>
          <p className="text-base text-white/40 mb-8">Hypixel proxy with game overlay and Lua scripting.</p>
          <Link href="/starfish/dashboard"
            className="inline-block px-6 py-2.5 rounded-md bg-white/[0.08] hover:bg-white/[0.14] text-white/70 text-sm font-medium transition-colors">
            Dashboard
          </Link>
        </div>
      </section>

      <section className="w-full max-w-4xl mx-auto px-6 py-12">
        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
          <FeatureCard title="Game Overlay" description="Rendered directly into Minecraft via DLL injection. See stats, tags, and custom UI without alt-tabbing." />
          <FeatureCard title="Lua Scripting" description="Write plugins in Lua with full access to game state, packets, and the overlay renderer." />
          <FeatureCard title="Cross-Platform" description="Native support for Windows, Linux, and macOS." />
        </div>
      </section>

      <section className="w-full max-w-4xl mx-auto px-6 py-12">
        <div className="rounded-lg border border-white/[0.08] bg-[rgba(0,0,0,0.5)] p-6">
          <h2 className="text-xl font-bold mb-2">Coming Soon</h2>
          <p className="text-sm text-white/40">
            Starfish is currently in development. Stay tuned for updates.
          </p>
        </div>
      </section>

      <StarfishFooter />
    </div>
  );
}


function FeatureCard({ title, description }: { title: string; description: string }) {
  return (
    <div className="rounded-lg border border-white/[0.08] bg-[rgba(0,0,0,0.5)] p-5">
      <h3 className="text-sm font-semibold text-white/70 mb-1.5">{title}</h3>
      <p className="text-[12px] text-white/35 leading-relaxed">{description}</p>
    </div>
  );
}
