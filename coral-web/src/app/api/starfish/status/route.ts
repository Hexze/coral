import { NextResponse } from "next/server";
import { cookies } from "next/headers";

const API_URL = process.env.CORAL_API_URL || "http://localhost:8000";

type DiscordUser = { id: string; username: string; avatar: string | null };
type LicenseStatus = { has_license: boolean };
type ReleaseInfo = { version: string; platforms: Record<string, { filename: string; size: number }> };

export async function GET() {
  const token = (await cookies()).get("sf_token")?.value;
  if (!token) return NextResponse.json({ authenticated: false });

  const [userRes, licenseRes, releaseRes] = await Promise.all([
    fetch("https://discord.com/api/v10/users/@me", {
      headers: { Authorization: `Bearer ${token}` },
    }),
    fetch(`${API_URL}/api/v1/starfish/license/check`, {
      headers: { Authorization: `Bearer ${token}` },
    }).catch(() => null),
    fetch(`${API_URL}/api/v1/starfish/download/info`).catch(() => null),
  ]);

  if (!userRes.ok) return NextResponse.json({ authenticated: false });

  const user = (await userRes.json()) as DiscordUser;
  const license = licenseRes?.ok ? ((await licenseRes.json()) as LicenseStatus) : null;
  const release = releaseRes?.ok ? ((await releaseRes.json()) as ReleaseInfo) : null;

  return NextResponse.json({
    authenticated: true,
    user: { id: user.id, username: user.username, avatar: user.avatar },
    has_license: license?.has_license ?? false,
    release: release ?? null,
  });
}
