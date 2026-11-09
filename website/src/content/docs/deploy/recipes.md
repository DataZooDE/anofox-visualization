---
title: Platform recipes
description: GitHub Pages, S3 and CloudFront, Docker Compose and systemd — the files are in the repository's deploy/ folder.
---

## GitHub Pages (a static site)

Build the site in a workflow and publish it with Pages; refresh it on a schedule:

```yaml
on: { push: { branches: [main] }, schedule: [{ cron: "0 5 * * *" }] }
jobs:
  site:
    runs-on: ubuntu-latest
    permissions: { pages: write, id-token: write }
    steps:
      - uses: actions/checkout@v5
      - run: ./dashboard --site dashboards/ public/ --data data/
      - uses: actions/upload-pages-artifact@v4
        with: { path: public }
      - uses: actions/deploy-pages@v4
```

Everything in `public/` is public. [Static sites →](../static-site/)

## S3 and CloudFront

`dashboard --site dashboards/ public/ --data data/`, then `aws s3 sync public/ s3://my-site/` behind a CloudFront distribution. Parquet in the bucket can be read by URL instead (`--data https://…`), with a CORS rule for the site's origin.

## Docker Compose (the locked server behind Caddy)

```sh
docker build -f deploy/Dockerfile -t anofox-dashboards .
docker compose -f deploy/compose.yaml up -d      # the server + Caddy for TLS
```

The image runs the locked server as an unprivileged user; dashboards, the server config and the data are mounted read-only. Publish the port on the host's loopback and let the proxy reach it; list the public host name in `hosts` (else `421`). [`deploy/`](https://github.com/DataZooDE/anofox-visualization/blob/master/deploy) · [Containers in detail →](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md#container)

## systemd

```sh
install -D -m 755 deploy/run-locked.sh /opt/anofox/run-locked.sh
install -m 755 duckdb anofox_visualization.duckdb_extension /opt/anofox/
install -D -m 644 serve.sql /etc/anofox/serve.sql
cp deploy/anofox-dashboards.service /etc/systemd/system/
systemctl daemon-reload && systemctl enable --now anofox-dashboards
```

A reverse proxy (Caddy or nginx, both in `deploy/`) terminates TLS and can do the login. [systemd, proxies, replicas, sizing →](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/deployment.md)
