# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-service-v0.6.0...templar-gateway-service-v0.7.0) - 2026-09-11

### Added

- *(gateway)* [**breaking**] oracle.updateLazer accepts multiple feeds (ENG-676) ([#615](https://github.com/Templar-Protocol/contracts/pull/615))

## [0.6.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-service-v0.5.1...templar-gateway-service-v0.6.0) - 2026-09-01

### Added

- *(gateway)* [**breaking**] validate registry init args against ABI (ENG-463) ([#607](https://github.com/Templar-Protocol/contracts/pull/607))

### Fixed

- *(gateway)* settle a transaction the chain never recorded (ENG-477) ([#611](https://github.com/Templar-Protocol/contracts/pull/611))

## [0.5.1](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-service-v0.5.0...templar-gateway-service-v0.5.1) - 2026-08-25

### Added

- *(gateway)* add tx.batch, a generic multi-action write op (ENG-650) ([#603](https://github.com/Templar-Protocol/contracts/pull/603))

## [0.5.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-service-v0.4.0...templar-gateway-service-v0.5.0) - 2026-08-24

### Added

- *(gateway)* [**breaking**] factor out the shared deploy-from-registry op structure (ENG-466) ([#594](https://github.com/Templar-Protocol/contracts/pull/594))
- *(registry)* [**breaking**] register an existing global contract by code hash (ENG-631) ([#596](https://github.com/Templar-Protocol/contracts/pull/596))

## [0.4.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-service-v0.3.0...templar-gateway-service-v0.4.0) - 2026-08-07

### Added

- *(gateway)* [**breaking**] oracle.updatePyth fetches its own payload (ENG-462) ([#586](https://github.com/Templar-Protocol/contracts/pull/586))

## [0.3.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-service-v0.2.0...templar-gateway-service-v0.3.0) - 2026-08-04

### Added

- *(gateway-core)* [**breaking**] serialize the write path per access key (ENG-530) ([#561](https://github.com/Templar-Protocol/contracts/pull/561))
- *(gateway)* [**breaking**] opt into borsh governance proposals (ENG-558) ([#565](https://github.com/Templar-Protocol/contracts/pull/565))

## [0.2.0](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-service-v0.1.1...templar-gateway-service-v0.2.0) - 2026-08-03

### Added

- *(proxy-oracle-governance)* [**breaking**] reflexive vs. target-function-call operations (ENG-516) ([#527](https://github.com/Templar-Protocol/contracts/pull/527))

## [0.1.1](https://github.com/Templar-Protocol/contracts/compare/templar-gateway-service-v0.1.0...templar-gateway-service-v0.1.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Performance

- *(testing)* faster local sandbox gate + CI pool robustness ([#526](https://github.com/Templar-Protocol/contracts/pull/526))
