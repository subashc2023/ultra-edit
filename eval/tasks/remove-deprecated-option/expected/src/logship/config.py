"""Settings shared by the command line and the transport."""

from dataclasses import dataclass


@dataclass
class ShipperConfig:
    """Everything logship needs to reach the collector and report results."""

    endpoint: str
    batch_size: int = 500
    ca_file: str | None = None
    client_cert: str | None = None
    tls_server_name: str | None = None
    legacy_output: bool = False
    retry_delays: tuple[float, ...] = (1.0, 2.0, 5.0, 10.0)

    def __post_init__(self):
        if not self.endpoint.startswith("https://"):
            raise ValueError(f"endpoint must be an https:// URL: {self.endpoint}")
        if self.batch_size < 1:
            raise ValueError(f"batch_size must be at least 1, got {self.batch_size}")
