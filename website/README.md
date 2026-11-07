# The website (#1017)

Astro Starlight; deployed by `.github/workflows/pages.yml` to `/next/`.

```sh
cd website && npm ci
npm run dev        # http://localhost:4321/anofox-visualization/next/ — reloads on every edit
```

`npm run dev` also serves the builder (`../web`) at `/next/app/`, so "open in the builder" links work
locally; it needs `web/pkg` (`wasm-pack build --target web --out-dir web/pkg --release --no-default-features --features wasm`).
The chart pages are generated when the server starts (`npm run gen` regenerates them);
`node scripts/gen-charts.mjs --thumbs` re-renders their pictures with the dashboard CLI.
