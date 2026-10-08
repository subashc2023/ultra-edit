# Settings

Every setting has a default in `src/orders/settings.py`. To override one, set an
environment variable named `ORDERS_<SETTING>` before the service starts; for
example, `ORDERS_READ_TIMEOUT_S=15` gives slow upstreams more time.

| Setting | Default | Meaning |
| --- | --- | --- |
| `DB_POOL_SIZE` | `10` | Database connections kept open per worker. |
| `DB_POOL_RECYCLE_S` | `1800` | Seconds before a pooled connection is replaced. |
| `DB_STATEMENT_TIMEOUT_MS` | `5000` | Milliseconds before a query is cancelled. |
| `RETRY_LIMIT` | `3` | Attempts per request, including the first. |
| `RETRY_BACKOFF_S` | `0.5` | Seconds before the first retry; doubled after each failure. |
| `RETRY_BACKOFF_MAX_S` | `8.0` | Longest wait between two attempts. |
| `RETRY_STATUS_CODES` | `(429, 502, 503, 504)` | Responses that are retried. |
| `CONNECT_TIMEOUT_S` | `3.0` | Seconds for the TCP and TLS handshake. |
| `READ_TIMEOUT_S` | `10.0` | Seconds to wait for the first byte of a response. |
| `WRITE_TIMEOUT_S` | `10.0` | Seconds to finish sending a request body. |
| `POOL_TIMEOUT_S` | `5.0` | Seconds to wait for a free connection. |
| `CACHE_TTL_S` | `300` | Seconds a cached order stays fresh. |
| `WEBHOOK_RETRY_LIMIT` | `3` | Deliveries per event before it is parked. |
| `WEBHOOK_TIMEOUT_S` | `10.0` | Seconds to wait for a webhook receiver. |
| `LOG_LEVEL` | `"INFO"` | Lowest level that is logged. |

Settings that are not listed here are internal and may change without notice.
