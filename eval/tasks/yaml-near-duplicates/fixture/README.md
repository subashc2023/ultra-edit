# shop-deploy

Deployment manifests for the shop.

- `deploy/k8s/app.yaml`: staging and prod Deployments (containers `api`,
  `worker`, `scheduler`) and their Services, as one multi-document file.
- `deploy/k8s/configmap.yaml`: prod nginx config, login banner (`motd.txt`;
  its lines end with trailing spaces on purpose), and the scheduler's cron list.
- `deploy/k8s/staging-values.yaml`: staging overlay values.
- `deploy/compose.yaml`: local stack; shared settings live in the
  `x-defaults` anchor.

Both environments currently run `registry.example.com/shop/worker:1.8.2` with a
`memory: 512Mi` limit.
