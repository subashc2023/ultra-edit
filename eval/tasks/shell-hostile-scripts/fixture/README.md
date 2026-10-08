# acme-deploy

Release and deployment tooling for the Acme web fleet.

## Deploying

    make deploy ENV=staging

`scripts/deploy.sh` reads `REGION` from the environment and falls back to
`${REGION:-us-east-1}`. Set `DEPLOY_TOKEN` before running it; in CI it comes
from `${{ secrets.DEPLOY_TOKEN }}`.

The manifest written to each host looks like this:

```yaml
version: 3.2.0
commit: 1a2b3c4
region: us-east-1
```

It is produced by a `cat > "$MANIFEST" <<EOF ... EOF` heredoc, so any
`$(...)` inside it runs on the machine that deploys, not on the host.

## Rolling back

    make rollback ENV=staging PREVIOUS=3.1.4
