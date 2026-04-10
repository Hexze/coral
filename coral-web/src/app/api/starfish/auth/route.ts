import { NextResponse } from "next/server";

const DISCORD_CLIENT_ID = process.env.STARFISH_DISCORD_CLIENT_ID || "";
const SITE_URL = process.env.SITE_URL || "https://coral.urchin.gg";

export async function GET() {
  const params = new URLSearchParams({
    client_id: DISCORD_CLIENT_ID,
    redirect_uri: `${SITE_URL}/api/starfish/callback`,
    response_type: "code",
    scope: "identify",
  });

  return NextResponse.redirect(`https://discord.com/oauth2/authorize?${params}`);
}
