# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and this project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- GitHub Release packaging now publishes `aetherd` and `aether-mcp` binaries for
  x64 and arm64 across Linux, macOS, and Windows targets.

### Changed

- CI now runs explicit required jobs for `fmt`, `clippy`, and workspace tests.
- Release workflow now supports both semver tag pushes (`v*.*.*`) and manual
  dispatch with a required `tag` input.
