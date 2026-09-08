# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Report zero assets when the adapter has no Blend supply position, keeping vault refreshes live
  before first allocation and after a complete exit.

## [1.0.1](https://github.com/Templar-Protocol/contracts/compare/templar-soroban-blend-adapter-v1.0.0...templar-soroban-blend-adapter-v1.0.1) - 2026-08-03

### Added

- *(release)* automate per-crate releases and version contract artifacts (ENG-522) ([#528](https://github.com/Templar-Protocol/contracts/pull/528))

### Fixed

- *(soroban)* allow account admins for Blend adapters
