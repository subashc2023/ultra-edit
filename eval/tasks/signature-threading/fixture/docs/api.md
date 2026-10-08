# API reference

## `fetch_json(url, *, retries=3)`

Fetches `url` and decodes the JSON body. Network errors (`OSError`) are retried
with a short linear back-off; the last error is re-raised.

| Parameter | Type | Default | Description |
| --- | --- | --- | --- |
| `url` | `str` | required | Absolute URL to fetch. |
| `retries` | `int` | `3` | Attempts before giving up. |

Example:

```python
from pkg.net import fetch_json

data = fetch_json("https://api.tidepool.example/v2/products/A-100", retries=5)
```

## `fetch_json_cached(url, cache, *, retries=3)`

Same as `fetch_json`, but returns `cache[url]` when present and stores the
result otherwise.

| Parameter | Type | Default | Description |
| --- | --- | --- | --- |
| `url` | `str` | required | Absolute URL to fetch. |
| `cache` | `dict` | required | Mapping of URL to decoded body. |
| `retries` | `int` | `3` | Attempts before giving up. |

```python
data = fetch_json_cached("https://api.tidepool.example/v2/categories/tea", cache, retries=5)
```

## `CatalogClient.fetch_jsonl(path)`

Streams a JSON Lines export one decoded object at a time. It uses the client's
own `timeout_s` and does not retry.

## TypeScript: `fetchJson<T>(url, retries = 3)`

The storefront helper in `web/api.ts` mirrors `fetch_json`.
