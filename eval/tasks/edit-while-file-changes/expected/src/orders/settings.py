"""Runtime settings for the orders service.

Values here are the defaults. Each one can be overridden at startup through an
environment variable named ORDERS_<SETTING>, which `orders.config.load` reads.
"""

from __future__ import annotations

SERVICE_NAME = "orders"
API_VERSION = "2024-06-01"

# Database
DB_POOL_SIZE = 10
DB_POOL_RECYCLE_S = 1800
DB_STATEMENT_TIMEOUT_MS = 5000

# Region routing
DEFAULT_REGION = "eu-west-1"
FALLBACK_REGIONS = ("eu-central-1", "us-east-1")

# Retry policy
RETRY_LIMIT = 5  # attempts per request, including the first
RETRY_BACKOFF_S = 0.5  # doubled after each failed attempt
RETRY_JITTER = True  # randomize each backoff by up to half
RETRY_BACKOFF_MAX_S = 8.0
RETRY_STATUS_CODES = (429, 502, 503, 504)

# Timeouts, in seconds
CONNECT_TIMEOUT_S = 3.0  # TCP and TLS handshake
DNS_TIMEOUT_S = 2.0  # name resolution
READ_TIMEOUT_S = 20.0  # waiting for the first byte
WRITE_TIMEOUT_S = 10.0
POOL_TIMEOUT_S = 5.0

# Caching
CACHE_TTL_S = 300
CACHE_MAX_ENTRIES = 10_000
CACHE_NEGATIVE_TTL_S = 30

# Webhooks
WEBHOOK_SIGNING_ALGORITHM = "sha256"
WEBHOOK_RETRY_LIMIT = 3  # deliveries per event before it is parked
WEBHOOK_TIMEOUT_S = 10.0

# Logging
LOG_LEVEL = "INFO"
LOG_FORMAT = "json"
LOG_SAMPLE_RATE = 1.0


def as_dict() -> dict[str, object]:
    """Every setting above, by name, for the /debug/settings endpoint."""
    return {name: value for name, value in globals().items() if name.isupper()}
