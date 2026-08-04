use crate::types::*;
use near_sdk::env;

pub(crate) fn effective_sub_lifecycle(
    sub: &SubAccountEntry,
    tla: &TlaEntry,
    retraction_notice_ns: u64,
    grace_period_ns: u64,
) -> LifecycleStatus {
    if matches!(tla.lifecycle(grace_period_ns), LifecycleStatus::Reclaimable) {
        return LifecycleStatus::Reclaimable;
    }
    if let Some(retraction_at) = sub.retraction_at {
        let elapsed_at = retraction_at.saturating_add(retraction_notice_ns);
        if env::block_timestamp() >= elapsed_at {
            return LifecycleStatus::Reclaimable;
        }
    }
    sub.lifecycle(grace_period_ns)
}
