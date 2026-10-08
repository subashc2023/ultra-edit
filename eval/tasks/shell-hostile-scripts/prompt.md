Move the Acme deployment from `us-east-1` to `eu-central-1` and tighten up the release pipeline. Make the 12 changes below in three files: `scripts/deploy.sh`, `.github/workflows/release.yml`, and `Makefile`.

Each code block below shows one complete line (or a group of complete lines) exactly as it appears in the file, including its leading spaces, except that `Makefile` recipe lines are shown without their leading tab. Every `$`, quote, backslash, and backtick is a literal character in the file; nothing is expanded or escaped by this prompt. All three files use Unix LF line endings.

## `scripts/deploy.sh`

1. Replace the line

```
REGION="${REGION:-us-east-1}"
```

with

```
REGION="${REGION:-eu-central-1}"
```

The line `FALLBACK_REGION="${FALLBACK_REGION:-us-west-2}"` stays unchanged.

2. In the `log()` function, replace the line (indented with two spaces)

```
  printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*" >&2
```

with (still indented with two spaces)

```
  printf '[%s] [%s] %s\n' "$(date -u +%H:%M:%S)" "$ENVIRONMENT" "$*" >&2
```

The `printf` lines in `die()` and at the end of the file stay unchanged.

3. Inside the `cat > "$MANIFEST" <<EOF` heredoc, replace the line

```
deployed_by: $(whoami)@$(hostname -s)
```

with

```
deployed_by: ${DEPLOY_USER:-$(whoami)}@$(hostname -f)
```

4. That heredoc ends with a line that is exactly `EOF` (the only such line in the file). Directly after that `EOF` line, insert these four new lines, each with no indentation:

```
cat >> "$MANIFEST" <<EOF
checksum: $(sha256sum "dist/acme-${VERSION}.tar.gz" | cut -d' ' -f1)
notes: "see 'CHANGELOG.md' for ${VERSION}"
EOF
```

The new block's last line is also exactly `EOF`. The blank line that followed the original `EOF` line now follows the new block, so a single blank line still separates it from the `python3 - "$MANIFEST" <<'PY'` line.

5. Inside the `<<'PY'` heredoc, replace the line

```
print(json.dumps({k: v for k, v in pairs}, indent=2))
```

with

```
print(json.dumps({k: v for k, v in pairs}, indent=2, sort_keys=True))
```

6. In the health-check failure branch, replace the line (indented with two spaces)

```
  echo 'Health check failed; run '\''make rollback'\'' to revert!' >&2
```

with (still indented with two spaces)

```
  echo 'Health check failed in '"$REGION"'; run '\''make rollback PREVIOUS=<version>'\'' to revert!' >&2
```

## `.github/workflows/release.yml`

7. In the `Upload bundle` step, replace the line (indented with ten spaces)

```
          name: acme-${{ matrix.os }}-${{ github.ref_name }}
```

with (still indented with ten spaces)

```
          name: acme-${{ matrix.os }}-${{ github.sha }}
```

8. In the `env:` block of the `deploy` job, replace the line (indented with six spaces)

```
      REGION: ${{ vars.DEPLOY_REGION || 'us-east-1' }}
```

with (still indented with six spaces)

```
      REGION: ${{ vars.DEPLOY_REGION || 'eu-central-1' }}
```

9. In the same `env:` block, replace the line (indented with six spaces)

```
      DEPLOY_TOKEN: ${{ secrets.DEPLOY_TOKEN }}
```

with (still indented with six spaces)

```
      DEPLOY_TOKEN: ${{ secrets.PROD_DEPLOY_TOKEN || secrets.DEPLOY_TOKEN }}
```

The `API_TOKEN: ${{ secrets.DEPLOY_TOKEN }}` line in the `Smoke test` step stays unchanged.

10. In the `download-artifact` step, replace the line (indented with ten spaces)

```
          name: acme-ubuntu-22.04-${{ github.ref_name }}
```

with (still indented with ten spaces)

```
          name: acme-ubuntu-22.04-${{ github.sha }}
```

Every other `${{ github.ref_name }}` (in the `Notify` step) and every `${{ matrix.os }}` elsewhere stays unchanged.

## `Makefile`

Recipe lines start with exactly one tab character. Keep that tab on both changed lines.

11. In the `deploy` recipe, replace the recipe line

```
REGION=$${REGION:-us-east-1} ./scripts/deploy.sh $(ENV)
```

with

```
REGION=$${REGION:-eu-central-1} ./scripts/deploy.sh $(ENV) $(DEPLOY_ARGS)
```

12. In the `cache` recipe, replace the recipe line

```
mkdir -p $$HOME/.cache/acme && cp $(TARBALL) $$HOME/.cache/acme/
```

with

```
mkdir -p $$HOME/.cache/acme/$(VERSION) && cp $(TARBALL) $$HOME/.cache/acme/$(VERSION)/
```

The `CACHE_DIR ?= $$HOME/.cache/acme` variable line stays unchanged.

`README.md`, `scripts/lib.sh` (which has its own `REGION="${REGION:-us-east-1}"` line, `log()` function, and `EOF` heredoc), `.github/workflows/ci.yml`, and every other file stay exactly as they are.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
