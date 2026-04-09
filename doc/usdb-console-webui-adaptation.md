# USDB Console WebUI Adaptation

## Goal

This document records how `usdb` should adapt the frontend patterns from
`/home/bucky/work/buckyos_webdesktop` for the `USDB Control Console`.

The intent is:

- keep `buckyos_webdesktop` as the upstream reference
- freeze a local adaptation guide inside `usdb`
- copy only the minimum stable baseline resources into `usdb`
- avoid coupling the `usdb` build and Docker flow to the full upstream project

## Upstream Reference

Primary upstream files currently used as the design and implementation source:

- `buckyos_webdesktop/skills/webui-prototype/SKILL.md`
- `buckyos_webdesktop/package.json`
- `buckyos_webdesktop/src/index.css`
- `buckyos_webdesktop/src/i18n/provider.tsx`
- `buckyos_webdesktop/src/i18n/dictionaries.ts`
- `buckyos_webdesktop/src/components/DesktopVisualTokens.ts`

These files define the preferred frontend stack, design tokens, and i18n shape.

## Adaptation Rules

### 1. Keep Control Plane As The Only Data Entry

`web/usdb-console` must continue to read from `usdb-control-plane` only.

It should not:

- talk directly to multiple backend RPCs from the browser
- read Docker state directly
- duplicate aggregation logic already owned by `usdb-control-plane`

### 2. Migrate Toward The Upstream Frontend Stack

The target frontend stack for the next implementation phase is:

- React 19
- TypeScript
- Vite
- Tailwind CSS 3
- Lucide React
- `react-hook-form` + `zod` when user input is introduced
- local i18n provider based on the upstream shape

The previous static `index.html + app.js + styles.css` implementation should be
treated as legacy reference only. The React/Vite console is now the default
runtime entry.

### 3. English-First API, Frontend-Owned Localization

The backend should keep returning structured English-first status values.

The frontend owns:

- locale selection
- text translation
- display labels for bootstrap steps, service states, and section headings

### 4. Reuse The Upstream Visual Language, Not A New One

The `USDB Control Console` should align with the BuckyOS visual system:

- `--cp-*` color and surface tokens
- `shell-panel`, `shell-pill`, `shell-kicker` primitives
- Plus Jakarta Sans + Sora typography pairing
- desktop-shell style spacing and focus treatment

It should not invent another unrelated dashboard design language.

## Minimum Local Resource Set

The following baseline resources are copied into `usdb` as a frozen local
reference set:

- `web/shared/buckyos-webui-baseline/README.md`
- `web/shared/buckyos-webui-baseline/tokens.css`
- `web/shared/buckyos-webui-baseline/i18n-provider.tsx`
- `web/shared/buckyos-webui-baseline/i18n-dictionaries.example.ts`
- `web/shared/buckyos-webui-baseline/visual-tokens.ts`

These files are not yet wired into the current control console runtime. They
exist so the next migration step can build against a stable local baseline.

## Why Copy A Small Baseline Instead Of Depending On The Full Upstream Project

Directly depending on the entire `buckyos_webdesktop` project would create:

- build coupling across repositories
- version drift between runtime and design reference
- extra Docker and packaging complexity
- unclear ownership when `usdb-console` starts evolving independently

Copying only the minimum stable baseline gives `usdb`:

- local reference assets
- a clear migration target
- freedom to evolve the console without importing the entire upstream app

## What Is Intentionally Not Copied Yet

This adaptation does not yet copy:

- the full React app shell
- MUI-specific layout code
- page-specific application components
- the complete upstream dictionary set
- desktop window management code

Those belong to the full BuckyOS desktop product, not to the `usdb` console.

## Next Implementation Step

The next frontend migration step should:

1. scaffold a React/Vite version of `web/usdb-console`
2. import the local baseline tokens and i18n shape from `web/shared`
3. preserve the current `usdb-control-plane` API contract
4. reimplement the current overview page using the upstream shell style

Current migration workspace:

- `web/usdb-console-app`

This app now hosts the runtime control-console implementation and produces the
served `dist/` assets consumed by `usdb-control-plane`.
