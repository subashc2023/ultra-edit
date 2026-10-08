# Service catalog

Services that receive traffic directly. The source of truth is
`config/services.toml`; this page mirrors its `port`, `protocol`, `owner`,
and `region` keys.

| Service           | Port | Protocol | Owner          | Region         |
| ----------------- | ---- | -------- | -------------- | -------------- |
| auth-api          | 8080 | http     | team-auth      | ap-southeast-2 |
| auth-gateway      | 8080 | http     | team-auth      | us-east-1      |
| auth-indexer      | 9090 | grpc     | team-auth      | us-east-1      |
| billing-api       | 8080 | http     | team-billing   | us-east-1      |
| billing-gateway   | 8080 | http     | team-billing   | eu-west-1      |
| billing-indexer   | 9090 | grpc     | team-billing   | eu-west-1      |
| catalog-api       | 8080 | http     | team-catalog   | eu-west-1      |
| catalog-gateway   | 8080 | http     | team-catalog   | ap-southeast-2 |
| catalog-indexer   | 9090 | grpc     | team-catalog   | us-east-1      |
| checkout-api      | 8080 | http     | team-checkout  | us-east-1      |
| checkout-gateway  | 8080 | http     | team-checkout  | us-east-1      |
| checkout-indexer  | 9090 | grpc     | team-checkout  | ap-southeast-2 |
| inventory-api     | 8080 | http     | team-inventory | ap-southeast-2 |
| inventory-gateway | 8080 | http     | team-inventory | eu-west-1      |
| inventory-indexer | 9090 | grpc     | team-inventory | us-east-1      |
| ledger-api        | 8080 | http     | team-ledger    | eu-west-1      |
| ledger-gateway    | 8080 | http     | team-ledger    | us-east-1      |
| ledger-indexer    | 9090 | grpc     | team-ledger    | us-east-1      |
| notify-api        | 8080 | http     | team-notify    | us-east-1      |
| notify-gateway    | 8080 | http     | team-notify    | ap-southeast-2 |
| notify-indexer    | 9090 | grpc     | team-notify    | ap-southeast-2 |
| orders-api        | 8080 | http     | team-orders    | ap-southeast-2 |
| orders-gateway    | 8080 | http     | team-orders    | eu-west-1      |
| orders-indexer    | 9090 | grpc     | team-orders    | us-east-1      |
| payments-api      | 8080 | http     | team-payments  | us-east-1      |
| payments-gateway  | 8080 | http     | team-payments  | us-east-1      |
| payments-indexer  | 9090 | grpc     | team-payments  | eu-west-1      |
| pricing-api       | 8080 | http     | team-pricing   | eu-west-1      |
| pricing-gateway   | 8080 | http     | team-pricing   | ap-southeast-2 |
| pricing-indexer   | 9090 | grpc     | team-pricing   | us-east-1      |
| profile-api       | 8080 | http     | team-profile   | ap-southeast-2 |
| profile-gateway   | 8080 | http     | team-profile   | us-east-1      |
| profile-indexer   | 9090 | grpc     | team-profile   | ap-southeast-2 |
| reports-api       | 8080 | http     | team-reports   | us-east-1      |
| reports-gateway   | 8080 | http     | team-reports   | eu-west-1      |
| reports-indexer   | 9090 | grpc     | team-reports   | eu-west-1      |
| search-api        | 8080 | http     | team-search    | eu-west-1      |
| search-gateway    | 8080 | http     | team-search    | us-east-1      |
| search-indexer    | 9090 | grpc     | team-search    | us-east-1      |
| shipping-api      | 8080 | http     | team-shipping  | us-east-1      |
| shipping-gateway  | 8080 | http     | team-shipping  | us-east-1      |
| shipping-indexer  | 9090 | grpc     | team-shipping  | ap-southeast-2 |
| tax-api           | 8080 | http     | team-tax       | ap-southeast-2 |
| tax-gateway       | 8080 | http     | team-tax       | eu-west-1      |
| tax-indexer       | 9090 | grpc     | team-tax       | us-east-1      |

Batch roles (`admin`, `cache`, `export`, `scheduler`, `stream`, `sync`,
`webhooks`, `worker`) are not listed here. Ask `team-search` about search
relevance and `team-auth` about tokens.
