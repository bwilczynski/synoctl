# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/bwilczynski/synoctl/compare/v0.1.0...v0.1.1) - 2026-10-16

### Added

- read the password from a password_command

## [0.1.0](https://github.com/bwilczynski/synoctl/releases/tag/v0.1.0) - 2026-10-15

### Added

- config init and config check
- shell completion with NAS IDs
- synoctl command tree with table and JSON output
- one-screen NAS status with verdicts
- docker containers, images, networks and image update tracking
- system info, utilization, storage volumes and Hyper Backup tasks
- client for the DSM Web API with config, stored session and response cache

### Fixed

- drop restart count from container lists
- keep cached responses separate per DSM account
- hash cache file names so long requests are cached
