"""High-level client for the Tidepool catalog API."""

from __future__ import annotations

import json
import urllib.request

from pkg.net import fetch_json, fetch_json_cached

BASE_URL = "https://api.tidepool.example/v2"


class CatalogClient:
    """Read-only access to products, stores, prices, and exports."""

    def __init__(self, base_url=BASE_URL, *, retries=3, timeout_s=30.0):
        self.base_url = base_url.rstrip("/")
        self.retries = retries
        self.timeout_s = timeout_s
        self._cache = {}

    def product(self, sku):
        return fetch_json(f"{self.base_url}/products/{sku}", retries=self.retries)

    def store_inventory(self, store_id, *, include_reserved=False):
        url = f"{self.base_url}/stores/{store_id}/inventory"
        if include_reserved:
            url += "?reserved=1"
        return fetch_json(
            url,
            retries=self.retries,
        )

    def prices(self, skus):
        loader = lambda sku: fetch_json(f"{self.base_url}/prices/{sku}", retries=self.retries)
        return {sku: loader(sku)["amount"] for sku in skus}

    def category(self, slug):
        return fetch_json_cached(f"{self.base_url}/categories/{slug}", self._cache, retries=self.retries)

    def fetch_jsonl(self, path):
        """Stream a JSON Lines export such as ``exports/products.jsonl``."""
        url = f"{self.base_url}/{path.lstrip('/')}"
        with urllib.request.urlopen(url, timeout=self.timeout_s) as resp:
            for raw in resp:
                if raw.strip():
                    yield json.loads(raw)
