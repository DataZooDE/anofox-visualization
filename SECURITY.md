# Security

## Reporting a vulnerability

Please report security problems privately, through GitHub's
[private vulnerability reporting](https://github.com/DataZooDE/anofox-visualization/security/advisories/new)
("Report a vulnerability" on the Security tab) — not in a public issue.

Include what is affected (the DuckDB extension, the locked dashboard server,
`serve`, the browser app), a way to reproduce it, and the impact you see. We
acknowledge reports within 3 working days, keep you informed while we fix
it, and publish a GitHub Security Advisory with the fixed release, crediting
you unless you prefer otherwise.

The trust model of the servers (what a viewer, a login and a dashboard author
can and cannot do) is in [docs/secure-serving.md](docs/secure-serving.md).

## Supported versions

Fixes go into the next release (date-based versions, `vYYYY.MM.DD`). Please
use the latest one; older releases are not patched.

## Verifying a release

Every release carries `SHA256SUMS`, its keyless Sigstore signature
`SHA256SUMS.sigstore.json`, an SPDX SBOM (`anofox-visualization-<tag>.spdx.json`)
and a GitHub build provenance attestation for every asset.

```sh
# the signature: made by this repository's release workflow on a v* tag
cosign verify-blob SHA256SUMS --bundle SHA256SUMS.sigstore.json \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp '^https://github.com/DataZooDE/anofox-visualization/\.github/workflows/extension\.yml@refs/tags/v'
# the files you downloaded
sha256sum --check --ignore-missing SHA256SUMS
# or, per file, the provenance
gh attestation verify anofox_visualization.v2026.11.2.duckdb-v1.5.6.linux_amd64.duckdb_extension \
  --repo DataZooDE/anofox-visualization
```

## How the code is checked

- CI on every change: tests, `clippy -D warnings`, the server and browser
  end-to-end suites.
- Weekly: CodeQL (JavaScript, Rust, workflows), osv-scanner (known
  vulnerabilities in `Cargo.lock` and `package-lock.json`), gitleaks (secrets
  in the history) and zizmor (the workflows) — `.github/workflows/security.yml`.
- GitHub Actions are pinned by commit; Dependabot proposes updates only for
  releases at least 7 days old.
- A release builds from a clean cache and is never re-tagged.
