# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-09-22

### Added

- `jev ask`, `jev noul`, `jev choice`, `jev score`, `jev example`, and `jev completions`.
- Text and JSON output, `--dry-run`, `noul --threshold`, verbose reporting, and `--debug`
  lines written as each attempt fails.
- Exit codes 0–7 for success, a missed threshold, usage errors, authentication, rejected
  requests, context length, other API or network failures, and local I/O.
- A response body is not printed unless `--debug` is set, except the short serde fragment that
  names what was unreadable in an otherwise successful reply. The crate does not quote the request
  or the API key.
- `--timeout` and `JEV_TIMEOUT` must be from 1 to 86400 seconds.
- A questions document with a repeated id is rejected. A leading UTF-8 BOM on the state or the
  document is ignored.
- Release binaries for Linux (x86_64 and aarch64, statically linked with musl) and macOS (arm64
  and x86_64), attached to the GitHub release as `.tar.gz` archives with SHA-256 checksums.
