//! The two reads background learning needs from the partitions (Learning v2,
//! design 09 §1.4), on the `Extraction` surface only.
//!
//! Neither is tool-reachable and neither records anything: `Extraction`'s
//! capabilities stay write-only (design §7a). L2 adds `learning_view`, the
//! known-notes lookup, beside them.

use super::{MemoryAccessContext, MemoryError, MemoryService, Surface};
use openalpaca_storage::memory_files::PartitionHealth;
use std::collections::HashSet;

impl MemoryService {
    fn require_extraction(ctx: &MemoryAccessContext) -> Result<(), MemoryError> {
        if ctx.surface != Surface::Extraction {
            return Err(MemoryError::invalid(
                "this read is background learning's own; no other surface may make it",
            ));
        }
        Ok(())
    }

    /// Refuses when a partition the chunk would write to is quarantined
    /// (design §2): checked **before** the model call, so no cost is spent on
    /// a window that cannot be written. The window waits for
    /// `memory doctor --repair`.
    pub(crate) fn learning_preflight(&self, ctx: &MemoryAccessContext) -> Result<(), MemoryError> {
        Self::require_extraction(ctx)?;
        let resolved = self.resolve(ctx)?;
        for partition in resolved.all() {
            if let PartitionHealth::Quarantined(reason) = partition.health() {
                return Err(MemoryError::Quarantined {
                    partition_key: partition.partition_key().to_string(),
                    reason,
                });
            }
        }
        Ok(())
    }

    /// Which of `keys` (`message:<id>`) an explicit forget suppressed in
    /// either partition of `ctx` (design §1.4(d), §6 *Forget contract*): a
    /// replayed window must never cite them again, or a forgotten note would
    /// come back.
    pub(crate) fn suppressed_sources(
        &self,
        ctx: &MemoryAccessContext,
        keys: &[String],
    ) -> Result<HashSet<String>, MemoryError> {
        Self::require_extraction(ctx)?;
        if keys.is_empty() {
            return Ok(HashSet::new());
        }
        let resolved = self.resolve(ctx)?;
        let snapshots: Vec<_> = resolved
            .all()
            .into_iter()
            .filter_map(|p| p.snapshot())
            .collect();
        Ok(keys
            .iter()
            .filter(|k| snapshots.iter().any(|s| s.is_source_suppressed(k)))
            .cloned()
            .collect())
    }
}
