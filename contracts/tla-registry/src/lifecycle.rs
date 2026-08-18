use crate::types::*;
use near_sdk::env;

pub(crate) fn effective_sub_lifecycle(
    sub: &SubAccountEntry,
    tla: &TlaEntry,
    retraction_notice_ns: u64,
    clock: &LifecycleClock,
    tla_suspended_until: u64,
) -> LifecycleStatus {
    if matches!(
        tla.lifecycle(clock, tla_suspended_until),
        LifecycleStatus::Reclaimable
    ) {
        return LifecycleStatus::Reclaimable;
    }
    if let Some(retraction_at) = sub.retraction_at {
        let elapsed_at = retraction_at
            .saturating_add(retraction_notice_ns)
            .max(clock.reclaim_floor_ns);
        if env::block_timestamp() >= elapsed_at {
            return LifecycleStatus::Reclaimable;
        }
    }
    sub.lifecycle(clock)
}
