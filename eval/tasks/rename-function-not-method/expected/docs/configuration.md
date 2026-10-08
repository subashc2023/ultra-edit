# Configuration

relay loads its settings from a JSON or YAML file when it starts. The default
path is `/etc/relay/settings.json`; pass `--config PATH` to load another file.
Environment variables that start with `RELAY_` override values from the file,
so `RELAY_LISTEN_PORT=9100` wins over `listen_port` in the file.

| Setting       | Default            | Meaning                                |
| ------------- | ------------------ | -------------------------------------- |
| `listen_host` | `127.0.0.1`        | Address the webhook listener binds to. |
| `listen_port` | `8080`             | Port the webhook listener binds to.    |
| `cache_dir`   | `/var/cache/relay` | Where the route table is cached.       |
| `plugins`     | (none)             | Plugins to load at startup, in order.  |
| `retry_limit` | `3`                | Forwarding attempts per event.         |

Keys that are not in the table are kept, unchanged, in `Settings.extra`.

## Loading settings from Python

`config.load_config()` returns a frozen `Settings` object. A missing file is not an
error; you get the defaults instead.

```python
from relay import config

settings = config.load_config("/srv/relay/settings.yaml")
print(settings.listen_port)
```

To pick up an edited file without a restart, send the process `SIGHUP`. The
service then calls `config.reload()`, which keeps the old settings if the new
file is invalid.

## Plugins

Each name in `plugins` is imported with `plugins.load()`, which looks for a
module of that name in the `relay_plugins` package and checks that it defines
`setup(settings)`. Plugins can use `Cache.load()` to read values the service
has cached, such as the route table.
