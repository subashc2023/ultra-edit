"""Built-in defaults for wayfetch.

The client, the command-line tool, and the config file loader all take their
defaults from this module, so each value is defined exactly once. The --help
text, the documentation, and examples/wayfetch.toml repeat some of them for
readers; keep those in step when a value changes here.
"""

# Timeouts, in seconds. The connect timeout covers DNS resolution, the TCP
# handshake, and the TLS handshake. The read timeout applies to each wait for
# response data, not to the response as a whole.
DEFAULT_CONNECT_TIMEOUT = 15.0
DEFAULT_READ_TIMEOUT = 10.0

# Retries. A request is attempted at most 1 + DEFAULT_MAX_RETRIES times. The
# pause before retry n is DEFAULT_BACKOFF_BASE * 2 ** n seconds, capped at
# DEFAULT_BACKOFF_MAX.
DEFAULT_MAX_RETRIES = 3
DEFAULT_BACKOFF_BASE = 0.5
DEFAULT_BACKOFF_MAX = 10.0

# Responses with these statuses are retried like connection errors. Every other
# status, including the other 5xx codes, is returned to the caller as it is.
RETRY_STATUSES = frozenset({429, 502, 503, 504})

# Connections kept open at once, across all hosts.
DEFAULT_MAX_CONNECTIONS = 100

CONFIG_PATH = "~/.config/wayfetch/config.toml"
USER_AGENT = "wayfetch/2.3.0 (+https://github.com/wayfetch/wayfetch)"
