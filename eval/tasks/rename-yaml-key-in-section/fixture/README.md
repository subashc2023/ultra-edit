# parcelhub

parcelhub is the parcel tracking API behind the customer portal. It polls the
carriers' tracking APIs, stores tracking events in PostgreSQL, and caches
lookups in Redis.

## Running locally

```console
$ python -m pip install -e .
$ parcelhub serve --env dev
```

There is no `config/dev.yaml`, so `--env dev` runs with the values in
`config/base.yaml`, which point at a local PostgreSQL and Redis.

## Deployments

| Environment | Settings files                                    |
| ----------- | ------------------------------------------------- |
| production  | `config/base.yaml`, `config/production.yaml`      |
| staging     | `config/base.yaml`, `config/staging.yaml`         |
| Windows     | `config/base.yaml`, `deploy/windows/service.yaml` |

Every key, its default, and the environment variable that overrides it are
described in [docs/configuration.md](docs/configuration.md).

## During an incident

If the database server runs out of connections, restart with a smaller
database pool size instead of waiting for a deploy:

```console
$ APP_DB_POOL_SIZE=10 parcelhub serve --env production
```

Remove the override once the incident is closed, and change the YAML file
instead if the smaller pool should stay.

## Tests

```console
$ python -m pytest
```
