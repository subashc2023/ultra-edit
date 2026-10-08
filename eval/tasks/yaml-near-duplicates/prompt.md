Roll out the shop's next prod changes across three YAML files. All three files indent with spaces only (no tabs). Many of the lines you change also appear elsewhere in the same file, so use the surrounding context to change only the occurrence described. Line numbers refer to the original files, before any edit. Each fenced block shows the lines to insert exactly as they appear in the file, with their full leading spaces.

`deploy/k8s/app.yaml` holds four YAML documents separated by `---` lines: the staging Deployment (`namespace: shop-staging`, lines 3-72), the prod Deployment (`namespace: shop-prod`, lines 74-143), the staging Service (lines 145-157), and the prod Service (lines 159-171). Both Deployments have the same three containers, `api`, `worker`, and `scheduler`, and the `scheduler` container runs the same `worker` image as the `worker` container.

1. `deploy/k8s/app.yaml`
   1. Staging Deployment, `api` container: on line 26, replace `image: registry.example.com/shop/api:2.4.1` with `image: registry.example.com/shop/api:2.5.0-rc.1`. The prod `api` image (line 97) stays `2.4.1`.
   2. Prod Deployment, `worker` container (`- name: worker` on line 113): on line 114, replace `image: registry.example.com/shop/worker:1.8.2` with `image: registry.example.com/shop/worker:1.8.3`. The prod `scheduler` image (line 132) and both staging images that use `worker:1.8.2` stay unchanged.
   3. Prod Deployment, `worker` container, `resources.limits`: on line 130, replace `memory: 512Mi` with `memory: 1Gi`. Its `requests` block and every other `limits` block (lines 41, 59, 72, 112, 143) keep `512Mi` and `256Mi` as they are.
   4. Prod Deployment, `api` container `ports` list: directly after line 100 (`              name: http`, the line after `- containerPort: 8080`), insert these two lines. The first has 12 spaces before `-`; the second has 14 spaces before `name:`.
```yaml
            - containerPort: 9090
              name: metrics
```
   5. Prod Deployment, `api` container `env` list: directly after line 105 (`              value: "10"`, the value of the `DB_POOL_SIZE` item), insert these two lines. The first has 12 spaces before `-`; the second has 14 spaces before `value:`.
```yaml
            - name: FEATURE_FLAGS_URL
              value: "http://flags.shop-prod.svc:8080"
```
   6. Prod Service: directly after its last line, line 171 (`      targetPort: http`), insert these three lines. The first has 4 spaces before `-`; the other two have 6 spaces before `port:` and `targetPort:`. The file still ends with a single newline, now after `      targetPort: metrics`. The staging Service stays unchanged.
```yaml
    - name: metrics
      port: 9090
      targetPort: metrics
```
2. `deploy/k8s/configmap.yaml`
   1. In the `nginx.conf` block scalar, inside `location /admin/ {` (line 18), replace `proxy_read_timeout 30s;` with `proxy_read_timeout 120s;` on line 20. The `location /api/ {` block keeps `proxy_read_timeout 30s;` on line 16.
   2. In the `motd.txt` block scalar, line 25 is 4 spaces, `Maintenance window: Sunday 02:00-04:00 UTC`, then exactly 3 trailing spaces. Replace `Sunday 02:00-04:00 UTC` with `Saturday 22:00-23:30 UTC`, keeping the 4 leading spaces and the 3 trailing spaces. Lines 24 (`Welcome to the shop prod cluster.`) and 26 (`On-call: #shop-oncall`) each end with exactly 2 trailing spaces, and line 27 (`Runbook: ...`) has none; leave all three unchanged.
3. `deploy/compose.yaml`
   1. In the `x-defaults: &defaults` anchor mapping, on line 8, replace `max-size: "10m"` with `max-size: "50m"`. The `<<: *defaults` alias lines stay as they are, and the `db` service's own `max-size: "10m"` on line 38 stays unchanged.
   2. In the `api` service's `ports` list, directly after line 16 (`      - "8080:8080"`), insert the line `      - "9091:9090"` with 6 spaces before `-`. The `worker` service's `- "9090:9090"` stays unchanged.

`deploy/k8s/staging-values.yaml` and `README.md` stay unchanged even though they contain the same image and memory lines, and `deploy/k8s/kustomization.yaml` stays unchanged.

Rules:
- Change only what is described above. Keep every other byte of every file unchanged, including line endings, indentation, trailing whitespace, and whether the file ends with a newline.
- Do not create, delete, rename, or reformat files.
- Do not run tests, builds, formatters, or other project tooling.
