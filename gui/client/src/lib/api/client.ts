import { L402Error, type L402Challenge } from "@/types/api";

/**
 * Robustly parses a WWW-Authenticate header challenge value into an L402Challenge object.
 *
 * Requirements satisfied:
 * - Supports modern `L402` and legacy `LSAT` schemes (case-insensitive scheme prefix).
 * - Supports `token` or `macaroon` key for the auth token.
 * - Supports `invoice` key for the payment invoice.
 * - Handles quoted (`token="..."`) and unquoted (`token=...`) parameter values.
 * - Supports arbitrary parameter ordering and arbitrary surrounding whitespace.
 * - Returns null for invalid or missing challenge headers without throwing unexpected errors.
 * - Never logs or exposes credentials.
 */
export function parseL402Header(authHeader?: string | null): L402Challenge | null {
  if (!authHeader) return null;

  const trimmed = authHeader.trim();
  let payload = "";

  if (trimmed.length >= 5 && trimmed.slice(0, 5).toUpperCase() === "L402 ") {
    payload = trimmed.slice(5).trim();
  } else if (trimmed.length >= 5 && trimmed.slice(0, 5).toUpperCase() === "LSAT ") {
    payload = trimmed.slice(5).trim();
  } else {
    return null;
  }

  let macaroon: string | undefined;
  let invoice: string | undefined;

  // Split on commas separating parameters: e.g. token="...", invoice="..."
  const params = payload.split(",");
  for (const rawParam of params) {
    const param = rawParam.trim();
    if (!param) continue;

    const eqIdx = param.indexOf("=");
    if (eqIdx === -1) continue;

    const key = param.slice(0, eqIdx).trim().toLowerCase();
    let val = param.slice(eqIdx + 1).trim();

    // Strip surrounding matching double quotes or single quotes if present
    if (
      (val.startsWith("\"") && val.endsWith("\"") && val.length >= 2) ||
      (val.startsWith("'") && val.endsWith("'") && val.length >= 2)
    ) {
      val = val.slice(1, -1).trim();
    }

    if (key === "token" || key === "macaroon") {
      macaroon = val;
    } else if (key === "invoice") {
      invoice = val;
    }
  }

  if (!macaroon || !invoice) {
    return null;
  }

  return {
    macaroon,
    invoice,
    parsedCaveats: parseMacaroonCaveats(macaroon),
  };
}

export function parseMacaroonCaveats(macaroonBase64: string): Record<string, string> {
  try {
    const jsonStr = atob(macaroonBase64);
    const parsed = JSON.parse(jsonStr);
    const caveats = parsed.caveats || [];
    const result: Record<string, string> = {};
    for (const cav of caveats) {
      if (typeof cav !== "string") continue;
      const parts = cav.split(" = ");
      if (parts.length === 2) {
        result[parts[0]] = parts[1];
      }
    }
    return result;
  } catch {
    return {};
  }
}

export async function fetchWithL402(url: string, options: RequestInit = {}) {
  const res = await fetch(url, options);

  if (res.status === 402) {
    const authHeader = res.headers.get("www-authenticate");
    const challenge = parseL402Header(authHeader);

    if (challenge) {
      throw new L402Error(challenge);
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
