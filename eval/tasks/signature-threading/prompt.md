Thread a new request timeout through the Tidepool client: from the CLI and the client class down to `fetch_json` and its inner `_request` call, plus the TypeScript helper, the tests, and the docs. Make exactly the 17 edits below in six files.

The Python files and `docs/api.md` use LF line endings and 4-space indentation in code. `web/api.ts` uses Windows CRLF line endings and 2-space indentation; keep CRLF on every line you change. Several look-alike names must not change: `fetch_json_cached`, `fetch_text`, the method `fetch_jsonl` in `pkg/client.py`, and the TypeScript method `fetchJsonl` in `web/api.ts`. The fenced code blocks below sit inside list items, so each of their lines carries 5 extra leading spaces of list indentation that are not part of the file text.

1. `pkg/net.py`
   - Replace `def fetch_json(url, *, retries=3):` with `def fetch_json(url, *, retries=3, timeout_s: float = 30.0):`
   - Inside `fetch_json`, replace `            return json.loads(_request(url))` (12 spaces of indentation) with `            return json.loads(_request(url, timeout_s=timeout_s))`. Leave the `_request(url)` call in `fetch_text` and the `fetch_json(url, retries=retries)` call in `fetch_json_cached` unchanged.
2. `pkg/client.py`
   - In `product`, replace `return fetch_json(f"{self.base_url}/products/{sku}", retries=self.retries)` with `return fetch_json(f"{self.base_url}/products/{sku}", retries=self.retries, timeout_s=self.timeout_s)`
   - In `store_inventory`, the `fetch_json(` call has one argument per line. Directly after its line `            retries=self.retries,` insert a new line `            timeout_s=self.timeout_s,` indented with 12 spaces, so the closing `        )` follows it.
   - In `prices`, replace `loader = lambda sku: fetch_json(f"{self.base_url}/prices/{sku}", retries=self.retries)` with `loader = lambda sku: fetch_json(f"{self.base_url}/prices/{sku}", retries=self.retries, timeout_s=self.timeout_s)`
   - Leave the `fetch_json_cached(...)` call in `category` unchanged even though it also ends with `retries=self.retries)`.
3. `pkg/cli.py`
   - Directly before the line `    parser.add_argument("command", choices=["product", "raw"])`, insert these six lines (the first and last are indented with 4 spaces, the middle four with 8 spaces):
     ```
         parser.add_argument(
             "--timeout",
             type=float,
             default=30.0,
             help="seconds to wait for each response (default: 30)",
         )
     ```
   - Replace `data = fetch_json(args.target, retries=args.retries)` with `data = fetch_json(args.target, retries=args.retries, timeout_s=args.timeout)`
   - Replace `client = CatalogClient(args.base_url, retries=args.retries)` with `client = CatalogClient(args.base_url, retries=args.retries, timeout_s=args.timeout)`
4. `tests/test_net.py`
   - Replace `request.assert_called_once_with("https://x.test/a")` with `request.assert_called_once_with("https://x.test/a", timeout_s=30.0)`. Leave `request.assert_called_once_with("https://x.test/t")` unchanged.
   - Replace `assert request.call_args_list == [mock.call("https://x.test/b")] * 2` with `assert request.call_args_list == [mock.call("https://x.test/b", timeout_s=30.0)] * 2`
5. `web/api.ts`
   - Replace `export async function fetchJson<T>(url: string, retries = 3): Promise<T> {` with `export async function fetchJson<T>(url: string, retries = 3, timeoutS = 30_000): Promise<T> {`
   - Replace `const res = await fetch(url, { headers: { Accept: "application/json" } });` with `const res = await fetch(url, { headers: { Accept: "application/json" }, signal: AbortSignal.timeout(timeoutS) });`. Leave the `fetch(...)` call inside `fetchJsonl` unchanged.
   - Replace ``return fetchJson<Product>(`${BASE_URL}/products/${sku}`);`` with ``return fetchJson<Product>(`${BASE_URL}/products/${sku}`, 3, 10_000);``
   - Replace ``return fetchJson<PriceQuote>(`${BASE_URL}/prices/${sku}`, retries);`` with ``return fetchJson<PriceQuote>(`${BASE_URL}/prices/${sku}`, retries, 5_000);``
6. `docs/api.md`
   - Change the first `##` heading line from
     ```
     ## `fetch_json(url, *, retries=3)`
     ```
     to
     ```
     ## `fetch_json(url, *, retries=3, timeout_s=30.0)`
     ```
   - In the `fetch_json` parameter table (the first table in the file), directly after the row ``| `retries` | `int` | `3` | Attempts before giving up. |`` insert the new row ``| `timeout_s` | `float` | `30.0` | Seconds to wait for each response. |``. The `fetch_json_cached` table has an identical `retries` row; leave that table unchanged.
   - Replace `data = fetch_json("https://api.tidepool.example/v2/products/A-100", retries=5)` with `data = fetch_json("https://api.tidepool.example/v2/products/A-100", retries=5, timeout_s=10.0)`. Leave the `fetch_json_cached(...)` example and the heading that starts with `## TypeScript:` unchanged.

`pkg/__init__.py`, `web/types.ts`, `README.md`, and `pyproject.toml` stay exactly as they are.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
