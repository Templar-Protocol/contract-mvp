# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.6.0](https://github.com/Templar-Protocol/contracts/compare/templar-market-contract-v1.5.3...templar-market-contract-v1.6.0) - 2026-09-11

### Added

- *(market)* expose supplier yield cash pool (ENG-665) ([#625](https://github.com/Templar-Protocol/contracts/pull/625))

### Fixed

- *(market)* reject excessive static yield withdrawals ([#627](https://github.com/Templar-Protocol/contracts/pull/627))

## [1.5.1](https://github.com/Templar-Protocol/contracts/compare/templar-market-contract-v1.5.0...templar-market-contract-v1.5.1) - 2026-08-24

### Fixed

- *(proxy-oracle)* remediate Halborn findings ([#595](https://github.com/Templar-Protocol/contracts/pull/595))

## [1.5.0](https://github.com/Templar-Protocol/contracts/compare/templar-market-contract-v1.4.2...templar-market-contract-v1.5.0) - 2026-08-04

### Added

- declarative market deployment — one spec, plan/apply, real preflight (ENG-537) ([#540](https://github.com/Templar-Protocol/contracts/pull/540))

## [1.4.1](https://github.com/Templar-Protocol/contracts/compare/templar-market-contract-v1.4.0...templar-market-contract-v1.4.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Deployments

- *(market)* iethfxrp-ixlmusdc alpha market ([#536](https://github.com/Templar-Protocol/contracts/pull/536))

### Performance

- *(testing)* faster local sandbox gate + CI pool robustness ([#526](https://github.com/Templar-Protocol/contracts/pull/526))
