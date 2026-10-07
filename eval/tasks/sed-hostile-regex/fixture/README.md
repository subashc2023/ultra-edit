# acme-storefront

Storefront API, web client, and edge configuration.

- `src/validate/` holds the server-side input patterns.
- `web/src/validate.ts` mirrors them for the browser.
- `docs/patterns.md` documents every pattern.
- `deploy/nginx/site.conf` is the edge server configuration.

Order IDs look like `ORD-1234567`. Phone numbers are shown as `(415) 555-0100`.
