# USDB Console Information Architecture

## Goal

This document defines the first route-based information architecture for the
React control console.

The console should no longer evolve as a single overview page. Instead, it
should behave as a small console application with a stable shell, top-level
navigation, and dedicated pages for each major operational domain.

## Primary Navigation

The first navigation tier should expose these top-level destinations:

- `Overview`
- `Services`
- `Bootstrap`
- `Protocol`
- `Me`

### Overview

Purpose:

- summarize overall service health
- show the most important chain and runtime metrics
- surface high-level bootstrap progress
- provide direct entry points into deeper pages

This page should stay summary-oriented. It is the landing page, not the place
for every detailed service field.

### Services

Purpose:

- provide the detailed read-only operational view for each runtime service
- replace the need to jump out to separate static browsers for common checks

The first implementation batch should include:

- `btc-node`
- `balance-history`
- `usdb-indexer`
- `ethw/geth`

The existing `balance-history-browser` and `usdb-indexer-browser` remain useful
as legacy explorers, but the long-term direction is to move their core value
into this page.

### Bootstrap

Purpose:

- show cold-start lifecycle state
- surface artifact presence and marker status
- show one-shot progress for snapshot loading, ETHW init, and future
  `sourcedao-bootstrap`

### Protocol

Purpose:

- later host miner-pass, energy, ranking, and protocol-state views

First batch:

- route exists
- page can be a controlled placeholder

### Me

Purpose:

- later host wallet-linked views for the current BTC / ETH identity
- show balance, miner pass ownership, and user-centric protocol state

First batch:

- route exists
- page can be a controlled placeholder

## Routing Strategy

The control console should use client-side routing, but for now it should avoid
server-side deep-link requirements.

Recommended first implementation:

- use hash routing
- keep `usdb-control-plane` static serving unchanged
- avoid any extra backend route fallback work during the first console-shell
  migration

Example route set:

- `#/overview`
- `#/services`
- `#/bootstrap`
- `#/protocol`
- `#/me`

## Page Composition Rules

### Shell

The app shell should own:

- console identity
- global refresh action
- locale selector
- top navigation

### Page Headers

Each page should render its own:

- page title
- page subtitle

This keeps the shell stable while allowing each section to explain its own
purpose.

### Data Loading

The first routing batch should keep using:

- `/api/system/overview`

This avoids coupling the first shell migration to new backend APIs.

Later iterations can split out:

- `/api/system/services`
- `/api/system/bootstrap`
- service-specific query APIs

## First Implementation Batch

The first batch should deliver:

1. top-level navigation shell
2. route-based pages for `Overview`, `Services`, and `Bootstrap`
3. placeholder pages for `Protocol` and `Me`
4. reuse of existing overview API data across all three implemented pages
5. explorer links preserved as fallback entry points while migration continues

## Deferred Work

The first batch intentionally does not include:

- wallet integration
- service-specific search forms
- deep protocol query tools
- admin actions
- route-specific backend APIs

These should come only after the shell and IA are stable.
