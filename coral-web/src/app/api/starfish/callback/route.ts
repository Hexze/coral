import { NextResponse } from "next/server";

const DISCORD_CLIENT_ID = process.env.STARFISH_DISCORD_CLIENT_ID || "";
const DISCORD_CLIENT_SECRET = process.env.STARFISH_DISCORD_CLIENT_SECRET || "";
const SITE_URL = process.env.SITE_URL || "https://coral.urchin.gg";

export async function GET(request: Request) {
  const code = new URL(request.url).searchParams.get("code");
  if (!code) return NextResponse.redirect(`${SITE_URL}/starfish/dashboard?error=no_code`);

  const tokenRes = await fetch("https://discord.com/api/v10/oauth2/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: DISCORD_CLIENT_ID,
      client_secret: DISCORD_CLIENT_SECRET,
      grant_type: "authorization_code",
      code,
      redirect_uri: `${SITE_URL}/api/starfish/callback`,
    }),
  });

  if (!tokenRes.ok) return NextResponse.redirect(`${SITE_URL}/starfish/dashboard?error=auth_failed`);

  const { access_token } = (await tokenRes.json()) as { access_token: string };

  const response = NextResponse.redirect(`${SITE_URL}/starfish/dashboard`);
  response.cookies.set("sf_token", access_token, {
    httpOnly: true,
    secure: true,
    sameSite: "lax",
    maxAge: 3600,
    path: "/",
  });

  return response;
}
