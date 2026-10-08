import type { JsonLine, PriceQuote, Product } from "./types";

const BASE_URL = "https://api.tidepool.example/v2";

export async function fetchJson<T>(url: string, retries = 3): Promise<T> {
  let lastError: unknown;
  for (let attempt = 0; attempt < retries; attempt++) {
    try {
      const res = await fetch(url, { headers: { Accept: "application/json" } });
      if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
      return (await res.json()) as T;
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError;
}

export function getProduct(sku: string): Promise<Product> {
  return fetchJson<Product>(`${BASE_URL}/products/${sku}`);
}

export function getPrice(sku: string, retries = 3): Promise<PriceQuote> {
  return fetchJson<PriceQuote>(`${BASE_URL}/prices/${sku}`, retries);
}

export class ExportReader {
  constructor(private readonly baseUrl = BASE_URL) {}

  async fetchJsonl(path: string): Promise<JsonLine[]> {
    const res = await fetch(`${this.baseUrl}/${path}`, { headers: { Accept: "application/x-ndjson" } });
    const text = await res.text();
    return text.split("\n").filter(Boolean).map((line) => JSON.parse(line) as JsonLine);
  }
}
