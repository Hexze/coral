import { NextResponse } from "next/server";
import { cookies } from "next/headers";

const API_URL = process.env.CORAL_API_URL || "http://localhost:8000";

export async function GET(request: Request) {
  const token = (await cookies()).get("sf_token")?.value;
  if (!token) return NextResponse.json({ error: "Not authenticated" }, { status: 401 });

  const platform = new URL(request.url).searchParams.get("platform") || "windows";

  const res = await fetch(
    `${API_URL}/api/v1/starfish/download/latest?platform=${encodeURIComponent(platform)}`,
    { headers: { Authorization: `Bearer ${token}` } },
  );

  if (!res.ok) {
    const body = await res.text().catch(() => "Download failed");
    return NextResponse.json({ error: body }, { status: res.status });
  }

  const filename =
    res.headers.get("content-disposition")?.match(/filename="?(.+?)"?$/)?.[1]
    ?? `starfish-${platform}`;

  return new NextResponse(res.body, {
    headers: {
      "Content-Type": "application/octet-stream",
      "Content-Disposition": `attachment; filename="${filename}"`,
    },
  });
}
