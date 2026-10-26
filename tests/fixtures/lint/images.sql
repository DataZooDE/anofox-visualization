-- @title Image lint fixture (#514)
-- @group Lint fixtures
--
-- One statement per `image/*` / `markdown/image-*` diagnostic, for the lint
-- parity tests (tests/lint_step.rs, tests/web/lint_test.mjs).

-- image/empty
SELECT NULL::IMAGE;

-- image/bad-src
SELECT 'javascript:alert(1)'::IMAGE, 'x'::ALT;

-- image/outside-folder
SELECT 'assets/../secret.png'::IMAGE, 'x'::ALT;

-- image/not-in-assets
SELECT 'img/ok.png'::IMAGE, 'x'::ALT;

-- image/svg-no-size
SELECT 'data:image/svg+xml,%3Csvg%3E%3C%2Fsvg%3E'::IMAGE, 'x'::ALT;

-- image/no-alt
SELECT 'https://example.com/x.png'::IMAGE;

-- image/many: 8 rows, one image each
SELECT 'https://example.com/' || i || '.png' ::IMAGE, 'img ' || i ::ALT
FROM range(8) t(i);

-- markdown/image-bad-src
SELECT 'Before ![bad](javascript:alert(1)) after'::MARKDOWN;

-- markdown/image-not-in-assets
SELECT 'See ![ok](img/ok.png) here'::MARKDOWN;

-- markdown/image-no-alt
SELECT 'See ![](https://example.com/x.png) here'::MARKDOWN;

-- markdown/placeholder-in-image
SELECT 'See ![x]({{col}}) here'::MARKDOWN;

-- clean: no diagnostic (relative under assets/, alt set)
SELECT 'assets/ok.svg'::IMAGE, 'a clean diagram'::ALT, 'Caption'::LABEL;
