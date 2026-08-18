use crate::types::SubAccountEntry;
use crate::{StorageKey, TlaRegistry};
use near_sdk::store::{IterableSet, LookupMap};
use near_sdk::AccountId;

type KeyIndex = LookupMap<AccountId, IterableSet<String>>;

fn index_add(
    index: &mut KeyIndex,
    id: &AccountId,
    key: &str,
    prefix: impl FnOnce(&AccountId) -> StorageKey,
) {
    index
        .entry(id.clone())
        .or_insert_with_key(|id| IterableSet::new(prefix(id)))
        .insert(key.to_string());
}

fn index_remove(index: &mut KeyIndex, id: &AccountId, key: &str) {
    if let Some(keys) = index.get_mut(id) {
        keys.remove(key);
    }
}

impl TlaRegistry {
    pub(crate) fn sub_account_insert(&mut self, key: String, entry: SubAccountEntry) {
        self.owner_index_add(&entry.owner, &key);
        index_add(&mut self.sub_accounts_by_tla, &entry.tla_id, &key, |tla| {
            StorageKey::SubAccountsByTlaInner { tla: tla.clone() }
        });
        self.sub_accounts.insert(key, entry);
    }

    pub(crate) fn sub_account_remove(&mut self, key: &str) -> Option<SubAccountEntry> {
        let removed = self.sub_accounts.remove(key)?;
        index_remove(&mut self.sub_accounts_by_owner, &removed.owner, key);
        index_remove(&mut self.sub_accounts_by_tla, &removed.tla_id, key);
        Some(removed)
    }

    pub(crate) fn sub_account_reassign(
        &mut self,
        key: &str,
        owner: &AccountId,
        payout: &AccountId,
    ) -> bool {
        let Some(sub) = self.sub_accounts.get_mut(key) else {
            return false;
        };
        let previous = std::mem::replace(&mut sub.owner, owner.clone());
        sub.payout_account = payout.clone();
        if previous != *owner {
            index_remove(&mut self.sub_accounts_by_owner, &previous, key);
            self.owner_index_add(owner, key);
        }
        true
    }

    fn owner_index_add(&mut self, owner: &AccountId, key: &str) {
        index_add(&mut self.sub_accounts_by_owner, owner, key, |owner| {
            StorageKey::SubAccountsByOwnerInner {
                owner: owner.clone(),
            }
        });
    }
}
