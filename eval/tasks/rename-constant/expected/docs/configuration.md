# Configuration

| Setting                | Default | Meaning                               |
| ---------------------- | ------- | ------------------------------------- |
| `RETRY_LIMIT`          | 3       | Retries after the first failed fetch. |
| `MAX_RETRIES_PER_HOST` | 2       | Retry budget shared by one host.      |
| `BACKOFF_SECONDS`      | 0.5     | Base delay between attempts.          |

Raising `RETRY_LIMIT` also raises the worst-case latency of `fetch`.
