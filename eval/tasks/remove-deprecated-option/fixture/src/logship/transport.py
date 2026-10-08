"""TLS connection to the collector and the batching send loop."""

import socket
import ssl
import time
from dataclasses import dataclass
from urllib.parse import urlsplit

DEFAULT_PORT = 6514


@dataclass
class ShipResult:
    shipped: int = 0
    failed: int = 0


def build_ssl_context(config):
    context = ssl.create_default_context(cafile=config.ca_file)
    context.minimum_version = ssl.TLSVersion.TLSv1_2

    if config.legacy_tls:
        context.minimum_version = ssl.TLSVersion.TLSv1
        context.set_ciphers("DEFAULT:@SECLEVEL=0")

    if config.client_cert:
        context.load_cert_chain(config.client_cert)
    return context


def connect(config):
    parts = urlsplit(config.endpoint)
    host = parts.hostname
    port = parts.port or DEFAULT_PORT
    context = build_ssl_context(config)
    sock = socket.create_connection((host, port), timeout=10)
    return context.wrap_socket(sock, server_hostname=config.tls_server_name or host)


def read_batches(paths, batch_size):
    batch = []
    for path in paths:
        with open(path, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                batch.append(line.rstrip("\n"))
                if len(batch) == batch_size:
                    yield batch
                    batch = []
    if batch:
        yield batch


def send_batch(config, conn, batch):
    payload = ("\n".join(batch) + "\n").encode("utf-8")
    for delay in config.retry_delays:
        try:
            conn.sendall(payload)
            return conn, True
        except OSError:
            conn.close()
            time.sleep(delay)
            try:
                conn = connect(config)
            except OSError:
                continue
    return conn, False


def ship(paths, config):
    result = ShipResult()
    conn = connect(config)
    try:
        for batch in read_batches(paths, config.batch_size):
            conn, sent = send_batch(config, conn, batch)
            if sent:
                result.shipped += len(batch)
            else:
                result.failed += len(batch)
    finally:
        conn.close()
    return result
