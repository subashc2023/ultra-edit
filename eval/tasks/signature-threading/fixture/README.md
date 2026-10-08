# tidepool

A small client for the Tidepool catalog API, with a matching TypeScript helper
for the storefront.

```python
from pkg.net import fetch_json

product = fetch_json("https://api.tidepool.example/v2/products/A-100", retries=3)
```

The command-line tool wraps the same call:

    python -m pkg.cli product A-100 --retries 5

See `docs/api.md` for the full reference.
