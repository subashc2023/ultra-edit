"""PostgreSQL connection pool for one parcelhub worker process."""

import logging

from psycopg_pool import ConnectionPool

log = logging.getLogger(__name__)


def open_pool(settings):
    """Open the connection pool for this worker.

    Every worker process calls this once at startup, so the database server
    sees up to settings.db_pool_size connections per worker. The pool opens
    one connection right away and the others on demand.
    """
    log.info(
        "opening database pool: up to %d connections, %.1fs wait for a free one",
        settings.db_pool_size,
        settings.db_pool_timeout,
    )
    return ConnectionPool(
        settings.db_url,
        min_size=1,
        max_size=settings.db_pool_size,
        timeout=settings.db_pool_timeout,
        kwargs={"options": f"-c statement_timeout={settings.db_statement_timeout_ms}"},
    )


def pool_usage(pool):
    """Return (connections in use, connections open) for the health endpoint."""
    stats = pool.get_stats()
    return stats["pool_size"] - stats["pool_available"], stats["pool_size"]
