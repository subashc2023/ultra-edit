# Configuration

wayfetch takes its settings from three places. A flag on the command line wins
over the config file, and the config file wins over the built-in defaults in
`wayfetch.defaults`. The config file is `~/.config/wayfetch/config.toml` unless
you name another one with `--config`; a missing default file is not an error.
[`examples/wayfetch.toml`](../examples/wayfetch.toml) lists every setting with
its default value.

## Settings

| Setting           | Table     | Flag                | Default | Meaning                                      |
| ----------------- | --------- | ------------------- | ------- | -------------------------------------------- |
| `connect_timeout` | `[http]`  | `--connect-timeout` | `10.0`  | Seconds allowed for DNS, TCP, and TLS.       |
| `read_timeout`    | `[http]`  | `--read-timeout`    | `10.0`  | Seconds allowed between bytes of a response. |
| `max_retries`     | `[retry]` | `--retries`         | `3`     | Retries after the first failed attempt.      |
| `backoff_max`     | `[retry]` | `--backoff-max`     | `10.0`  | Longest pause between attempts, in seconds.  |
| `max_connections` | `[pool]`  | `--max-connections` | `100`   | Connections kept open at once.               |
| `url`             | `[proxy]` | `--proxy`           | none    | Proxy that every request goes through.       |

Unknown tables and keys in the config file are an error, so a misspelled
setting never falls back to its default without a word. In Python, `Client`
takes the same settings as keyword arguments, with `proxy` for the proxy URL.

## Timeouts

A request can time out in two ways:

- The connect timeout covers resolving the host name, the TCP handshake, and
  the TLS handshake. It is 10 seconds unless you change it.
- The read timeout starts once the request has been sent and applies to each
  wait for data, not to the whole response, so a download that keeps receiving
  data never times out. It is 10 seconds unless you change it.

A request that times out is retried like any other connection error, up to
`max_retries` times, with pauses that double from 1 second up to `backoff_max`
(10 seconds by default). When the retries run out, the client raises
`FetchError`.

> Changed in 2.0: the default connect timeout was raised from 5 to 10 seconds,
> because TLS handshakes to servers on other continents often took longer than
> 5 seconds on mobile networks.

## Using a proxy

Set `url` in the `[proxy]` table, or pass `--proxy`, to send every request
through a proxy. For a local debugging proxy listening on port 10080:

```console
$ wayfetch --proxy http://127.0.0.1:10080 https://api.example.com/v1/status
```

wayfetch does not read `HTTPS_PROXY` or the other proxy environment variables,
so a proxy set for other tools never changes where wayfetch connects.
