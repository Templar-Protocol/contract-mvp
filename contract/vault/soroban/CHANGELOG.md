# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.1](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-runtime-v1.1.0...templar-soroban-runtime-v1.1.1) - 2026-09-11

### Fixed

- *(soroban)* report zero for empty Blend positions

## [1.1.0](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-runtime-v1.0.0...templar-soroban-runtime-v1.1.0) - 2026-08-03

### Added

- soroban curator kernel ([#378](https://github.com/Templar-Protocol/contracts/pull/378))
- *(soroban)* emit typed kernel contract events
- *(proxy-4626)* add Soroban ERC-4626 proxy
- *(soroban)* split idle and async withdrawals (#FIND-002 Nexus 39906066-2ebb-4b87-9e22-844ce7913a9c)
- *(proxy-curator)* add Soroban curator operations proxy
- *(soroban)* return typed execute receipts
- custodial market adapter (soroban)
- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Changed

- vault ergonomics ([#401](https://github.com/Templar-Protocol/contracts/pull/401))
- *(soroban-cli)* simplify redeployment guards (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* split command module ([#534](https://github.com/Templar-Protocol/contracts/pull/534))

### Documentation

- *(soroban)* document ttl keeper responsibilities
- *(soroban)* clarify queued withdrawal ergonomics
- *(soroban)* state atomic exit priority model
- *(soroban)* clarify immediate atomic exit wording
- fix rustdoc warnings and gate them in CI ([#532](https://github.com/Templar-Protocol/contracts/pull/532))
- *(soroban)* align Blend admin security guidance
- *(soroban)* clarify Blend admin eligibility

### ENG-484

- add custodial NAV report timestamps ([#529](https://github.com/Templar-Protocol/contracts/pull/529))
- expose vault version capabilities and replace curator proxy ([#530](https://github.com/Templar-Protocol/contracts/pull/530))

### Fixed

- *(proxy-4626)* type proxy view response
- *(proxy-4626)* use infallible proxy view conversion
- *(soroban)* make governance ttl renewal permissionless
- *(soroban)* reject partial idle queued withdrawals (#FIND-002 Nexus 39906066-2ebb-4b87-9e22-844ce7913a9c)
- *(soroban)* keep allocation finish from auto-withdrawing
- allow vault escrow under share-token whitelist
- *(curator-proxy)* decode foreign vault errors safely
- *(curator-proxy)* flatten downstream contract errors
- *(curator-proxy)* reject nonpositive allocations
- *(soroban-cli)* [**breaking**] require explicit adapter admin
- *(soroban)* preserve explicit deployment admins (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban-cli)* trim redeployment safeguards (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban)* close deployment authority gaps (Nexus 9e4e4f3c-9f48-44a1-92df-8d8300a69605)
- *(soroban)* allow account admins for Blend adapters
