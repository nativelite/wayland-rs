# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-11-16

### Added
- `wire`: the Wayland wire format. `Builder` writes a message, `Reader` reads
  its arguments, `header` and `validate` check bytes from a peer without
  panicking on any input; `Fixed` is the 24.8 fixed-point type; `Interface`,
  `Message` and `ArgDesc` describe an interface's requests and events.
- `Connection` (Linux): connects by `WAYLAND_SOCKET`, or `WAYLAND_DISPLAY`
  under `XDG_RUNTIME_DIR`; passes file descriptors both ways with
  `SCM_RIGHTS` (received ones close-on-exec and closed if never taken);
  allocates object ids and reuses them after `wl_display.delete_id`; knows
  compositor-created objects; buffers requests until `flush`; returns events
  as values from `next_event`, `wait_event` and `wait_event_timeout`;
  `roundtrip`; reports `wl_display.error` as `Error::Protocol`.
- `scan`: a protocol scanner. `parse` reads protocol XML, `generate` writes a
  Rust module with, per interface, its message tables, an object type whose
  methods send its requests, an `Event` enum with `decode`, and its enums as
  constants. `examples/scan.rs` runs it from the command line.
- `protocol::wayland` and `protocol::xdg_shell`: the scanner's output for the
  core protocol (wayland 1.24.0) and xdg-shell (wayland-protocols 1.45).
