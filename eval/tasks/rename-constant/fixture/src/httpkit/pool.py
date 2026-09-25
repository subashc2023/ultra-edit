from httpkit.settings import MAX_RETRIES_PER_HOST


def per_host_budget(hosts):
    return {host: MAX_RETRIES_PER_HOST for host in hosts}
