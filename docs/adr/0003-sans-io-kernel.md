# 0003. Sans-I/O kernel

- Status: Accepted
- Date: 2026-10-14

## Context

Shells run on bare metal (Embassy), Linux (epoll), Android (looper) and in
the browser (event loop). A kernel that blocks, spawns threads or calls an
OS would need a different implementation per platform.

## Decision

The kernel is a state machine without I/O. Shells call
`start(now)`, `handle(now, event)` and `tick(now)`; each call returns
`Effects` (refresh a region, wake me at time *t*, later: storage and network
requests) that the shell executes. Time is always passed in.

Plugins follow the same model: they read state through `Env` and request
changes with `Command`s, which the kernel applies after the plugin returns.

## Consequences

- One kernel for every platform; shells stay thin.
- Fully deterministic: tests and the headless emulator use simulated time.
- The command/effect model maps directly onto a message boundary, which the
  WebAssembly plugin backend needs.
- Long-running work must be split into steps (cooperative jobs) instead of
  blocking.
