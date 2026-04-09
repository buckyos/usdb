# BuckyOS WebUI Baseline

This directory stores the minimum local baseline copied from
`buckyos_webdesktop` for future `usdb-console` migration work.

Purpose:

- freeze a small, stable local reference
- avoid binding `usdb` directly to the full upstream project
- provide immediate assets for the next React/Vite migration

Current files:

- `tokens.css`
  - core `--cp-*` theme variables and shell primitives
- `i18n-provider.tsx`
  - reference i18n provider shape for React apps
- `i18n-dictionaries.example.ts`
  - minimal dictionary structure example
- `visual-tokens.ts`
  - lightweight status and tone helpers

These files are reference assets. They are not yet imported by the current
static `web/usdb-console` implementation.

