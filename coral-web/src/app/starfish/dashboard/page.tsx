"use client";

import { useEffect, useState } from "react";
import { StarfishNav, StarfishFooter } from "@/components/starfish/Layout";

type Platform = "windows" | "linux" | "macos";

type DashboardState =
  | { status: "loading" }
  | { status: "unauthenticated" }
  | { status: "ready"; user: DiscordUser; has_license: boolean; release: ReleaseInfo | null };

type DiscordUser = { id: string; username: string; avatar: string | null };
type ReleaseInfo = { version: string; platforms: Record<string, { filename: string; size: number }> };

const PLATFORM_LABELS: Record<Platform, string> = {
  windows: "Windows",
  linux: "Linux",
  macos: "macOS",
};

function detectPlatform(): Platform {
  if (typeof navigator === "undefined") return "windows";
  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes("mac")) return "macos";
  if (ua.includes("linux")) return "linux";
  return "windows";
}

function formatSize(bytes: number): string {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}


export default function DashboardPage() {
  const [state, setState] = useState<DashboardState>({ status: "loading" });
  const [selectedPlatform, setSelectedPlatform] = useState<Platform>(detectPlatform);

  useEffect(() => {
    fetch("/api/starfish/status")
      .then((r) => r.json())
      .then((data) => {
        if (!data.authenticated) {
          setState({ status: "unauthenticated" });
        } else {
          setState({ status: "ready", user: data.user, has_license: data.has_license, release: data.release });
        }
      })
      .catch(() => setState({ status: "unauthenticated" }));
  }, []);

  return (
    <div className="min-h-screen flex flex-col">
      <StarfishNav active="dashboard" />

      <div className="flex-1 flex items-start justify-center pt-28 px-6 pb-12">
        <div className="w-full max-w-lg">
          {state.status === "loading" && <LoadingCard />}
          {state.status === "unauthenticated" && <LoginCard />}
          {state.status === "ready" && (
            <AuthenticatedDashboard
              user={state.user}
              hasLicense={state.has_license}
              release={state.release}
              selectedPlatform={selectedPlatform}
              onPlatformChange={setSelectedPlatform}
            />
          )}
        </div>
      </div>

      <StarfishFooter />
    </div>
  );
}


function LoadingCard() {
  return (
    <Card>
      <div className="flex items-center justify-center py-8">
        <div className="text-sm text-white/30">Loading...</div>
      </div>
    </Card>
  );
}

function LoginCard() {
  return (
    <Card>
      <div className="text-center py-4">
        <img src="/starfish.png" alt="Starfish" width={40} height={40} className="mx-auto mb-4 pixelated" />
        <h1 className="text-xl font-bold mb-2">Starfish Dashboard</h1>
        <p className="text-sm text-white/40 mb-6">Sign in with Discord to access downloads.</p>
        <a href="/api/starfish/auth"
          className="inline-flex items-center gap-2 px-6 py-2.5 rounded-md bg-[#5865F2] hover:bg-[#4752C4] text-white text-sm font-medium transition-colors">
          <DiscordIcon />
          Sign in with Discord
        </a>
      </div>
    </Card>
  );
}


function AuthenticatedDashboard({
  user, hasLicense, release, selectedPlatform, onPlatformChange,
}: {
  user: DiscordUser;
  hasLicense: boolean;
  release: ReleaseInfo | null;
  selectedPlatform: Platform;
  onPlatformChange: (p: Platform) => void;
}) {
  const avatarUrl = user.avatar
    ? `https://cdn.discordapp.com/avatars/${user.id}/${user.avatar}.png?size=64`
    : `https://cdn.discordapp.com/embed/avatars/${Number(BigInt(user.id) >> 22n) % 6}.png`;

  return (
    <div className="space-y-4">
      <Card>
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <img src={avatarUrl} alt="" width={36} height={36} className="rounded-full" />
            <div>
              <div className="text-sm font-medium">{user.username}</div>
              <div className="text-[11px] text-white/30">
                {hasLicense
                  ? <span className="text-emerald-400/80">License Active</span>
                  : <span className="text-white/40">No License</span>}
              </div>
            </div>
          </div>
          <a href="/api/starfish/logout" className="text-[11px] text-white/25 hover:text-white/50 transition-colors">
            Sign out
          </a>
        </div>
      </Card>

      {hasLicense && release && (
        <Card>
          <div className="flex items-center justify-between mb-4">
            <h2 className="text-sm font-semibold">Download Starfish</h2>
            <span className="text-[11px] text-white/25">{release.version}</span>
          </div>

          <div className="flex gap-1.5 mb-4">
            {(Object.keys(PLATFORM_LABELS) as Platform[]).map((p) => (
              <button key={p} onClick={() => onPlatformChange(p)}
                className={`flex-1 py-1.5 rounded text-[11px] font-medium transition-colors ${
                  p === selectedPlatform
                    ? "bg-white/[0.10] text-white/70"
                    : "bg-white/[0.03] text-white/30 hover:text-white/50"
                }`}>
                {PLATFORM_LABELS[p]}
              </button>
            ))}
          </div>

          {release.platforms[selectedPlatform] ? (
            <a href={`/api/starfish/download?platform=${selectedPlatform}`}
              className="block w-full py-2.5 rounded-md bg-white/[0.08] hover:bg-white/[0.14] text-center text-sm text-white/70 font-medium transition-colors">
              Download for {PLATFORM_LABELS[selectedPlatform]}
              <span className="text-[11px] text-white/30 ml-2">
                ({formatSize(release.platforms[selectedPlatform].size)})
              </span>
            </a>
          ) : (
            <div className="py-2.5 rounded-md bg-white/[0.03] text-center text-sm text-white/30">
              Not available for {PLATFORM_LABELS[selectedPlatform]}
            </div>
          )}
        </Card>
      )}

      {hasLicense && !release && (
        <Card>
          <div className="text-center py-4">
            <p className="text-sm text-white/40">No releases available yet.</p>
          </div>
        </Card>
      )}

      {!hasLicense && (
        <Card>
          <div className="text-center py-4">
            <h2 className="text-sm font-semibold mb-2">No License</h2>
            <p className="text-[12px] text-white/35">You don't have a Starfish license.</p>
          </div>
        </Card>
      )}
    </div>
  );
}


function Card({ children }: { children: React.ReactNode }) {
  return (
    <div className="rounded-lg border border-white/[0.08] bg-[rgba(0,0,0,0.5)] p-5">
      {children}
    </div>
  );
}

function DiscordIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor">
      <path d="M20.317 4.37a19.791 19.791 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.21.375-.444.864-.608 1.25a18.27 18.27 0 0 0-5.487 0 12.64 12.64 0 0 0-.617-1.25.077.077 0 0 0-.079-.037A19.736 19.736 0 0 0 3.677 4.37a.07.07 0 0 0-.032.027C.533 9.046-.32 13.58.099 18.057a.082.082 0 0 0 .031.057 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028c.462-.63.874-1.295 1.226-1.994a.076.076 0 0 0-.041-.106 13.107 13.107 0 0 1-1.872-.892.077.077 0 0 1-.008-.128 10.2 10.2 0 0 0 .372-.292.074.074 0 0 1 .077-.01c3.928 1.793 8.18 1.793 12.062 0a.074.074 0 0 1 .078.01c.12.098.246.198.373.292a.077.077 0 0 1-.006.127 12.299 12.299 0 0 1-1.873.892.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.839 19.839 0 0 0 6.002-3.03.077.077 0 0 0 .032-.054c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.03zM8.02 15.33c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.956-2.419 2.157-2.419 1.21 0 2.176 1.095 2.157 2.42 0 1.333-.956 2.418-2.157 2.418zm7.975 0c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.956-2.419 2.157-2.419 1.21 0 2.176 1.095 2.157 2.42 0 1.333-.947 2.418-2.157 2.418z" />
    </svg>
  );
}

