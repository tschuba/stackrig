# Changelog

## [1.0.0](https://github.com/tschuba/stackrig/compare/v0.2.0...v1.0.0) (2026-11-25)


### ⚠ BREAKING CHANGES

* `strig import|migrate|proxy|route` and `strig move coolify` no longer work; use `strig coolify import|migrate|proxy|route|move`.

### Features

* add new, add, domain and mv for homelab stacks ([ad3a38d](https://github.com/tschuba/stackrig/commit/ad3a38d08804c3277bb1078cec15fa3bb07c7587))
* move Coolify commands under `strig coolify` ([977da5f](https://github.com/tschuba/stackrig/commit/977da5f34b0599d8e65e5b0f45d954f321b6e94f))

## [0.2.0](https://github.com/tschuba/stackrig/compare/v0.1.0...v0.2.0) (2026-11-25)


### Features

* add --only to import/migrate for partial Coolify migration ([#1](https://github.com/tschuba/stackrig/issues/1)) ([72bda8f](https://github.com/tschuba/stackrig/commit/72bda8f429a91c78678da592a8b88ecbc050b152))
* add `move coolify` to migrate one service/app in one go ([#4](https://github.com/tschuba/stackrig/issues/4)) ([8b34360](https://github.com/tschuba/stackrig/commit/8b3436054c14f5d6784159e158b5dde7ca286685))
* add homelab ops commands (ls, drift, deploy, update) ([#5](https://github.com/tschuba/stackrig/issues/5)) ([c9ea06f](https://github.com/tschuba/stackrig/commit/c9ea06fbc0a7c00ad2e52ec35cc3bda4c97a6678))
* build multi-platform binaries and update Homebrew tap on release ([#6](https://github.com/tschuba/stackrig/issues/6)) ([0d3a29f](https://github.com/tschuba/stackrig/commit/0d3a29fd926244012cb84fd47a9b89f72a1a28f7))
* route stacks behind Coolify's Traefik via an inner Traefik ([#3](https://github.com/tschuba/stackrig/issues/3)) ([952ddba](https://github.com/tschuba/stackrig/commit/952ddba75f764dceede57d73edc49dee70d1d9e8))
