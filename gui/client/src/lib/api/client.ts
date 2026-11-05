import { L402Error, L402Challenge } from "@/types/api";

function parseMacaroonCaveats(macaroonBase64: string): Record<string, string> {
  try {
    const jsonStr = atob(macaroonBase64);
    const parsed = JSON.parse(jsonStr);
    const caveats = parsed.caveats || [];
    const result: Record<string, string> = {};
    for (const cav of caveats) {
      const parts = cav.split(" = ");
      if (parts.length === 2) {
        result[parts[0]] = parts[1];
      }
    }
    return result;
  } catch (e) {
    return {};
  }
}

export async function fetchWithL402(url: string, options: RequestInit = {}) {
  const res = await fetch(url, options);

  if (res.status === 402) {
    const authHeader = res.headers.get("www-authenticate");
    if (authHeader && authHeader.startsWith("L402")) {
      const tokenMatch = authHeader.match(/token="([^"]+)"/);
      const invoiceMatch = authHeader.match(/invoice="([^"]+)"/);

      if (tokenMatch && invoiceMatch) {
        const macaroon = tokenMatch[1];
        const invoice = invoiceMatch[1];
        throw new L402Error({
          macaroon,
          invoice,
          parsedCaveats: parseMacaroonCaveats(macaroon)
        });
      }
    }
  }

  if (!res.ok) {
    let errMsg = `API Error: ${res.status}`;
    try {
      const errorJson = await res.json();
      if (errorJson?.error) {
        errMsg = errorJson.error;
      }
    } catch {
      // Fallback to status text
    }
    throw new Error(errMsg);
  }

  return res.json();
}
