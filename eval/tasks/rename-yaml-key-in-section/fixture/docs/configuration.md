# Configuration

parcelhub reads its settings from layered YAML files, in this order:

1. `config/base.yaml`, which every environment shares;
2. `config/<environment>.yaml` for the environment named with `--env`, if that
   file exists (`config/production.yaml` or `config/staging.yaml`);
3. the file passed with `--config`, if any. The Windows service passes
   `deploy/windows/service.yaml` this way.

Each file is merged on top of the ones before it, key by key, so a later file
lists only the keys it changes. Environment variables are applied last; see
[Environment variables](#environment-variables).

The tables below list every key by section. The Default column is the value
parcelhub uses when no file sets the key.

## Sections

### `service`

| Key       | Default        | Meaning                              |
| --------- | -------------- | ------------------------------------ |
| `name`    | `parcelhub`    | Name used in logs and metrics.       |
| `workers` | `4`            | Worker processes to start.           |
| `listen`  | `0.0.0.0:8080` | Address and port the API listens on. |

### `database`

| Key                    | Default | Meaning                                               |
| ---------------------- | ------- | ----------------------------------------------------- |
| `url`                  | (none)  | PostgreSQL connection URL. Required.                  |
| `pool_size`            | `10`    | Connections each worker keeps open to the database.   |
| `pool_timeout`         | `5.0`   | Seconds to wait for a free connection before failing. |
| `statement_timeout_ms` | `15000` | Longest one statement may run, in milliseconds.       |

Every worker process opens its own pool, so the database server sees up to
`database.pool_size` times `service.workers` connections from one host.
parcelhub refuses to start when that product is over 240, which leaves 10 of
the server's 250 connections for migrations and maintenance sessions.

A one-off reporting job that needs a larger pool than the web workers can
start with a file of its own:

```yaml
service:
  workers: 1
database:
  pool_size: 30
  pool_timeout: 30.0
```

### `cache`

| Key           | Default     | Meaning                                            |
| ------------- | ----------- | -------------------------------------------------- |
| `url`         | `memory://` | Redis URL, or `memory://` for an in-process cache. |
| `pool_size`   | `4`         | Redis connections each worker keeps open.          |
| `default_ttl` | `300`       | Seconds a cached tracking lookup stays valid.      |

Staging raises the cache pool for its nightly load tests:

```yaml
cache:
  pool_size: 6
```

### `http`

| Key         | Default | Meaning                                            |
| ----------- | ------- | -------------------------------------------------- |
| `pool_size` | `20`    | Connections each worker keeps to the carrier APIs. |
| `timeout`   | `10.0`  | Seconds to wait for a carrier API response.        |
| `retries`   | `3`     | Attempts per request before giving up.             |
| `proxy`     | (none)  | Proxy URL for outbound requests.                   |

### `logging`

| Key      | Default | Meaning                                         |
| -------- | ------- | ----------------------------------------------- |
| `level`  | `INFO`  | Lowest level written to the log.                |
| `format` | `json`  | `json` for log shippers, `text` for a terminal. |

## Environment variables

Environment variables override single keys after every file is merged. Use
them for incidents and one-off runs; permanent changes belong in the YAML
files.

- `APP_DB_URL` overrides `database.url`.
- `APP_DB_POOL_SIZE` overrides `database.pool_size`.
- `APP_CACHE_POOL_SIZE` overrides `cache.pool_size`.
- `APP_HTTP_POOL_SIZE` overrides `http.pool_size`.
- `APP_LOG_LEVEL` overrides `logging.level`.

## Reading settings from Python

```python
from parcelhub.settings import load_settings

settings = load_settings("production")
print(settings.db_pool_size, settings.cache_pool_size, settings.http_pool_size)
```

`load_settings` merges the files, applies the environment variables, checks
the connection budget, and returns a frozen `Settings` object. Its fields carry
a prefix for their section, such as `db_url`, `cache_default_ttl`, and
`http_timeout`, so code that uses them does not depend on the YAML layout.
