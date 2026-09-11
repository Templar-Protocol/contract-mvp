# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- *(soroban-cli)* resolve the fixed `soroban-v1.1.1` seven-asset catalog through an exact-length,
  SHA-256-verified release cache and fixed GitHub asset URLs

### Changed

- *(soroban-cli)* normal deployment now uses release/cache/workspace-seed precedence and fails
  closed instead of compiling implicitly
- *(soroban-cli)* persist the verified artifact cache in the operator image while retaining source,
  Rust toolchains, and `target` for explicit `--build`

### Migration

- *(soroban-cli)* pass bare `--build` to opt into checked-out-source compilation; the former
  implicit-build default and `--build false` form are no longer supported
- *(soroban-cli)* configure a custom cache root with the non-empty
  `TEMPLAR_SOROBAN_VAULT_ARTIFACT_CACHE` environment variable


## [0.2.0](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-vault-cli-v0.1.0...templar-soroban-vault-cli-v0.2.0) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Changed

- *(soroban-cli)* simplify redeployment guards (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* split command module ([#534](https://github.com/Templar-Protocol/contracts/pull/534))

### Documentation

- *(soroban)* align Blend admin security guidance

### ENG-484

- expose vault version capabilities and replace curator proxy ([#530](https://github.com/Templar-Protocol/contracts/pull/530))

### Fixed

- *(soroban-cli)* [**breaking**] require explicit adapter admin
- *(soroban)* preserve explicit deployment admins (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* harden fresh redeploy safety (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* trim redeployment safeguards (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban)* close deployment authority gaps (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* reject recorded governance on force-new
- *(soroban)* allow account admins for Blend adapters
