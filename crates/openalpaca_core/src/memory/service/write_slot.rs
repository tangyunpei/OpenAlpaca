//! One daemon-owned writer queue per partition (design §6, *Write protocol*)
//! and the quiesce hook a workspace move takes (design §3, *Workspace move*).
//!
//! A slot has three states. `Writing` serializes the service's own mutations
//! of one partition (two concurrent saves both land, one after the other —
//! fixture 14 step 1). `Held` is a quiesce: taken only while nothing is
//! writing, refused otherwise (`fixture:write_pending`), and it makes every
//! new writer wait until the guard drops (step 5 of the move). The slot key
//! is the partition's identity, so the lock is per partition, never global.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Writing,
    Held,
}

#[derive(Debug)]
struct Slot {
    state: Mutex<State>,
    cv: Condvar,
}

impl Slot {
    fn new() -> Self {
        Self {
            state: Mutex::new(State::Idle),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// The slots, keyed by partition identity.
#[derive(Debug, Default)]
pub(super) struct WriteSlots {
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

impl WriteSlots {
    fn slot(&self, key: &str) -> Arc<Slot> {
        let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        slots
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(Slot::new()))
            .clone()
    }

    /// Waits for the slot to be idle, then takes it for one mutation.
    pub(super) fn begin_write(&self, key: &str) -> WriteGuard {
        let slot = self.slot(key);
        {
            let mut state = slot.lock();
            while *state != State::Idle {
                state = slot.cv.wait(state).unwrap_or_else(|p| p.into_inner());
            }
            *state = State::Writing;
        }
        WriteGuard { slot }
    }

    /// Takes the slot for a workspace move: refused while a write is in
    /// flight (or the slot is already held), otherwise held until the guard
    /// drops, and every new writer waits meanwhile.
    pub(super) fn quiesce(&self, key: &str) -> Result<QuiesceGuard, ()> {
        let slot = self.slot(key);
        let mut state = slot.lock();
        if *state != State::Idle {
            return Err(());
        }
        *state = State::Held;
        drop(state);
        Ok(QuiesceGuard { slot })
    }

    /// Whether a write is in flight or the slot is held (tests, doctor).
    #[cfg(test)]
    pub(super) fn is_busy(&self, key: &str) -> bool {
        *self.slot(key).lock() != State::Idle
    }
}

/// Released on drop.
pub(super) struct WriteGuard {
    slot: Arc<Slot>,
}

impl Drop for WriteGuard {
    fn drop(&mut self) {
        *self.slot.lock() = State::Idle;
        self.slot.cv.notify_all();
    }
}

/// Step 5 of the workspace move: dropping it releases the held writers.
pub struct QuiesceGuard {
    slot: Arc<Slot>,
}

impl std::fmt::Debug for QuiesceGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("QuiesceGuard")
    }
}

impl Drop for QuiesceGuard {
    fn drop(&mut self) {
        *self.slot.lock() = State::Idle;
        self.slot.cv.notify_all();
    }
}
