# Command-line options

    logship --endpoint URL [options] PATH [PATH ...]

| Option                   | Default       | Description                                              |
| ------------------------ | ------------- | -------------------------------------------------------- |
| `--endpoint URL`         | required      | Collector to send to, as an `https://` URL.              |
| `--batch-size N`         | `500`         | Lines sent per request.                                  |
| `--ca-file PEM`          | system store  | Trust only the CA certificates in this bundle.           |
| `--client-cert PEM`      | none          | Certificate and private key to present to the collector. |
| `--tls-server-name NAME` | endpoint host | Name expected in the collector's certificate.            |
| `--legacy-output`        | off           | Print the 1.x one-line summary instead of JSON.          |

`--legacy-output` exists for monitoring scripts written against logship 1.x;
new scripts should parse the JSON summary.

## Exit status

`logship` exits with status 0 when every line was shipped and 1 when any batch
still failed after all retries. Usage errors exit with status 2.
