# logship

`logship` reads local log files and ships their lines to a central collector
over TLS. It sends lines in batches, retries a failed batch with backoff, and
prints a summary when it is done.

## Install

    pip install logship

## Usage

    logship --endpoint https://logs.example.com:6514 /var/log/app/*.log

Every option is listed in [docs/options.md](docs/options.md).

## TLS

logship checks the collector's certificate against the system trust store, or
against the bundle given with `--ca-file`, and requires TLS 1.2 or newer. Use
`--tls-server-name` when the certificate names a different host than the
endpoint, for example behind a load balancer.

### Connecting to old collectors

Collectors that only speak TLS 1.0 or 1.1 can still be reached with
`--legacy-tls`, which lowers the minimum protocol version and enables the weak
ciphers those collectors need. The option is deprecated and prints a warning
every time it is used; upgrade the collector instead.

## Output

By default logship prints a JSON object with the number of lines shipped and
failed. Pass `--legacy-output` to get the one-line `shipped=N failed=N` format
from logship 1.x, which some older monitoring scripts still parse.

## License

MIT
