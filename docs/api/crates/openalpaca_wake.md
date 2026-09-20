# `openalpaca_wake`

> Generated from source by `python3 scripts/gen_api_docs.py`.

## Overview

- Member path: `crates/openalpaca_wake`
- Entry: `crates/openalpaca_wake/src/lib.rs`

- Wake module
- Produces [`WakeEvent`]s from time-based schedules and external watchers (e.g. filesystem).

## Modules

- `manager` (crates/openalpaca_wake/src/manager/mod.rs)
- `models` (crates/openalpaca_wake/src/models.rs)
- `scheduler` (crates/openalpaca_wake/src/scheduler/mod.rs)
- `watcher` (crates/openalpaca_wake/src/watcher/mod.rs)

## Re-exports

- `pub use manager::WakeManager;`
- `pub use models::ScheduledTask;`
- `pub use scheduler::WakeScheduler;`
- `pub use watcher::EventWatcher;`
- `pub use watcher::filesystem::{FileWatchHandle, FilesystemWatcher};`
- `pub use openalpaca_api::events::WakeEvent;`

## Related Links

- [API Index](../README.md)
