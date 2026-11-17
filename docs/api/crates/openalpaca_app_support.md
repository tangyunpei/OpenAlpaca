# `openalpaca_app_support`

> Generated from source by `python3.11 scripts/gen_api_docs.py`.

## Overview

- Member path: `crates/openalpaca_app_support`
- Entry: `crates/openalpaca_app_support/src/lib.rs`

- OpenAlpaca's app support: what the daemon, the CLI and the app shell share
- to find, start, wait for and judge a daemon (Q05, DU-27 (a)).
- - [`paths`]: the home store's root and its own paths — `state/` and what
- lives in it (the database's and `discovery.json`'s names, the singleton
- lock, the logs with their rotation and tail, the caches, the backups),
- `config/`, `plugins/`, the root's README and `.layout`.
- - [`discovery`]: `discovery.json` — its types, its writer and reader, the
- token's issue and rotation, the singleton lock.
- - [`daemon_lifecycle`]: whether a daemon is running, and whether it is
- really gone yet.
- - [`protocol`]: the wire protocol's numbers, the starting state's budget
- and the decision whether a client uses the daemon a discovery file names.
- A leaf crate: it names no other workspace crate. `openalpaca_storage`
- (the content stores, the projects, the database) imports it, and nothing
- here re-exports or is re-exported (no `pub use` keeps an old path alive).

## Modules

- `daemon_lifecycle` (crates/openalpaca_app_support/src/daemon_lifecycle.rs)
- `discovery` (crates/openalpaca_app_support/src/discovery/mod.rs)
- `paths` (crates/openalpaca_app_support/src/paths.rs)
- `protocol` (crates/openalpaca_app_support/src/protocol/mod.rs)

## Re-exports

- No root-level `pub use` re-exports found.

## Related Links

- [API Index](../README.md)
