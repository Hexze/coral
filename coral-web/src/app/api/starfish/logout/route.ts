import { NextResponse } from "next/server";

const SITE_URL = process.env.SITE_URL || "https://coral.urchin.gg";

export async function GET() {
  const response = NextResponse.redirect(`${SITE_URL}/starfish/dashboard`);
  response.cookies.delete("sf_token");
  return response;
}
