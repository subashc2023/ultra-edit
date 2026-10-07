export interface Product {
  sku: string;
  name: string;
  priceCents: number;
}

export interface PriceQuote {
  sku: string;
  amount: number;
  currency: "EUR" | "USD";
}

export type JsonLine = Record<string, unknown>;
