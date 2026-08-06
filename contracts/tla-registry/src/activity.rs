use crate::events::Event;
use crate::types::ActivityRecord;
use crate::{TlaRegistry, ACTIVITY_CAPACITY};
use near_sdk::env;

impl TlaRegistry {
    pub(crate) fn emit_activity(&mut self, event: Event) {
        if let Some((name, account)) = event.activity_entry() {
            self.push_activity(name, account);
        }
        event.emit();
    }

    fn push_activity(&mut self, event: &str, account: String) {
        let record = ActivityRecord {
            event: event.to_string(),
            account,
            block_height: env::block_height(),
            block_timestamp: env::block_timestamp(),
        };
        if self.recent_activity.len() < ACTIVITY_CAPACITY {
            self.recent_activity.push(record);
            return;
        }
        self.recent_activity.replace(self.activity_cursor, record);
        self.activity_cursor = (self.activity_cursor + 1) % ACTIVITY_CAPACITY;
    }
}
