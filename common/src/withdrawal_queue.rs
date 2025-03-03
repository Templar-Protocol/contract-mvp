use std::num::NonZeroU32;

use near_sdk::{collections::LookupMap, env, near, AccountId, BorshStorageKey, IntoStorageKey};

use crate::asset::BorrowAssetAmount;

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct QueueNode {
    account_id: AccountId,
    amount: BorrowAssetAmount,
    prev: Option<NonZeroU32>,
    next: Option<NonZeroU32>,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct WithdrawalQueue {
    prefix: Vec<u8>,
    length: u32,
    is_locked: bool,
    next_queue_node_id: NonZeroU32,
    queue: LookupMap<NonZeroU32, QueueNode>,
    queue_head: Option<NonZeroU32>,
    queue_tail: Option<NonZeroU32>,
    entries: LookupMap<AccountId, NonZeroU32>,
}

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
enum StorageKey {
    Queue,
    Entries,
}

impl WithdrawalQueue {
    pub fn new(prefix: impl IntoStorageKey) -> Self {
        let prefix = prefix.into_storage_key();
        macro_rules! key {
            ($k:ident) => {
                [prefix.clone(), StorageKey::$k.into_storage_key()].concat()
            };
        }
        Self {
            prefix: prefix.clone(),
            length: 0,
            is_locked: false,
            next_queue_node_id: NonZeroU32::MIN,
            queue: LookupMap::new(key!(Queue)),
            queue_head: None,
            queue_tail: None,
            entries: LookupMap::new(key!(Entries)),
        }
    }

    #[inline]
    pub fn len(&self) -> u32 {
        self.length
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub fn get(&self, account_id: &AccountId) -> Option<BorrowAssetAmount> {
        self.entries
            .get(account_id)
            .and_then(|node_id| self.queue.get(&node_id))
            .map(|queue_node| queue_node.amount)
    }

    pub fn contains(&self, account_id: &AccountId) -> bool {
        self.entries.contains_key(account_id)
    }

    fn mut_existing_node<T>(
        &mut self,
        node_id: NonZeroU32,
        f: impl FnOnce(&mut QueueNode) -> T,
    ) -> T {
        if self.is_locked && Some(node_id) == self.queue_head {
            env::panic_str("Cannot mutate withdrawal queue head while queue is locked.");
        }

        let mut node = self
            .queue
            .get(&node_id)
            .unwrap_or_else(|| env::panic_str("Inconsistent state"));
        let r = f(&mut node);
        self.queue.insert(&node_id, &node);
        r
    }

    pub fn peek(&self) -> Option<(AccountId, BorrowAssetAmount)> {
        if let Some(node_id) = self.queue_head {
            let QueueNode {
                account_id, amount, ..
            } = self
                .queue
                .get(&node_id)
                .unwrap_or_else(|| env::panic_str("Inconsistent state"));
            Some((account_id, amount))
        } else {
            None
        }
    }

    /// # Errors
    /// - If the queue is already locked.
    /// - If the queue is empty.
    pub fn try_lock(
        &mut self,
    ) -> Result<(AccountId, BorrowAssetAmount), error::WithdrawalQueueLockError> {
        if self.is_locked {
            return Err(error::AlreadyLockedError.into());
        }

        if let Some(peek) = self.peek() {
            self.is_locked = true;
            Ok(peek)
        } else {
            Err(error::EmptyError.into())
        }
    }

    pub fn unlock(&mut self) {
        self.is_locked = false;
    }

    /// Only pops if:
    /// 1. Queue is non-empty.
    /// 2. Queue is locked.
    ///
    /// Unlocks the queue.
    pub fn try_pop(&mut self) -> Option<(AccountId, BorrowAssetAmount)> {
        if !self.is_locked {
            env::panic_str("Withdrawal queue must be locked to pop.");
        }

        self.is_locked = false;

        if let Some(node_id) = self.queue_head {
            let QueueNode {
                account_id,
                amount,
                next,
                ..
            } = self
                .queue
                .remove(&node_id)
                .unwrap_or_else(|| env::panic_str("Inconsistent state"));
            self.queue_head = next;
            if let Some(next_id) = next {
                self.mut_existing_node(next_id, |next| next.prev = None);
            } else {
                self.queue_tail = None;
            }
            self.entries.remove(&account_id);
            self.length -= 1;
            Some((account_id, amount))
        } else {
            None
        }
    }

    /// If the queue is locked, accounts can only be removed if they are not
    /// at the head of the queue.
    pub fn remove(&mut self, account_id: &AccountId) -> Option<BorrowAssetAmount> {
        if self.is_locked && self.queue_head == self.entries.get(account_id) {
            env::panic_str("Cannot remove head while withdrawal queue is locked.");
        }

        if let Some(node_id) = self.entries.remove(account_id) {
            let node = self
                .queue
                .remove(&node_id)
                .unwrap_or_else(|| env::panic_str("Inconsistent state"));

            if let Some(next_id) = node.next {
                self.mut_existing_node(next_id, |next| next.prev = node.prev);
            } else {
                self.queue_tail = node.prev;
            }

            if let Some(prev_id) = node.prev {
                self.mut_existing_node(prev_id, |prev| prev.next = node.next);
            } else {
                self.queue_head = node.next;
            }

            self.length -= 1;

            Some(node.amount)
        } else {
            None
        }
    }

    #[allow(clippy::missing_panics_doc)]
    pub fn insert_or_update(&mut self, account_id: &AccountId, amount: BorrowAssetAmount) {
        if let Some(node_id) = self.entries.get(account_id) {
            // update existing
            self.mut_existing_node(node_id, |node| node.amount = amount);
        } else {
            // add new
            let node_id = self.next_queue_node_id;
            {
                #![allow(clippy::unwrap_used)]
                // assume the collection never processes more than u32::MAX items
                self.next_queue_node_id = self.next_queue_node_id.checked_add(1).unwrap();
            }

            if let Some(tail_id) = self.queue_tail {
                self.mut_existing_node(tail_id, |tail| tail.next = Some(node_id));
            }
            let node = QueueNode {
                account_id: account_id.clone(),
                amount,
                prev: self.queue_tail,
                next: None,
            };
            if self.queue_head.is_none() {
                self.queue_head = Some(node_id);
            }
            self.queue_tail = Some(node_id);
            self.queue.insert(&node_id, &node);
            self.entries.insert(account_id, &node_id);
            self.length += 1;
        }
    }

    pub fn iter(&self) -> WithdrawalQueueIter {
        WithdrawalQueueIter {
            withdrawal_queue: self,
            next_node_id: self.queue_head,
        }
    }

    pub fn get_status(&self) -> WithdrawalQueueStatus {
        let depth = self
            .iter()
            .map(|(_, amount)| amount.as_u128())
            .sum::<u128>()
            .into();
        WithdrawalQueueStatus {
            depth,
            length: self.len(),
        }
    }

    pub fn get_request_status(&self, account_id: &AccountId) -> Option<WithdrawalRequestStatus> {
        if !self.contains(account_id) {
            return None;
        }

        let mut depth = 0.into();
        for (index, (current_account, amount)) in self.iter().enumerate() {
            if &current_account == account_id {
                return Some(WithdrawalRequestStatus {
                    // The queue's length is u32, so this will never truncate.
                    #[allow(clippy::cast_possible_truncation)]
                    index: index as u32,
                    depth,
                    amount,
                });
            }

            depth.join(amount);
        }

        unreachable!()
    }
}

impl<'a> IntoIterator for &'a WithdrawalQueue {
    type IntoIter = WithdrawalQueueIter<'a>;
    type Item = (AccountId, BorrowAssetAmount);

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct WithdrawalQueueIter<'a> {
    withdrawal_queue: &'a WithdrawalQueue,
    next_node_id: Option<NonZeroU32>,
}

impl Iterator for WithdrawalQueueIter<'_> {
    type Item = (AccountId, BorrowAssetAmount);

    fn next(&mut self) -> Option<Self::Item> {
        let next_node_id = self.next_node_id?;
        let r = self
            .withdrawal_queue
            .queue
            .get(&next_node_id)
            .unwrap_or_else(|| env::panic_str("Inconsistent state"));
        self.next_node_id = r.next;
        Some((r.account_id, r.amount))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct WithdrawalRequestStatus {
    pub index: u32,
    pub depth: BorrowAssetAmount,
    pub amount: BorrowAssetAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json])]
pub struct WithdrawalQueueStatus {
    pub depth: BorrowAssetAmount,
    pub length: u32,
}

pub mod error {
    use thiserror::Error;

    #[derive(Error, Debug)]
    #[error("The withdrawal queue is already locked")]
    pub struct AlreadyLockedError;

    #[derive(Error, Debug)]
    #[error("The withdrawal queue is empty")]
    pub struct EmptyError;

    #[derive(Error, Debug)]
    #[error("The withdrawal queue could not be locked: {}", .0)]
    pub enum WithdrawalQueueLockError {
        #[error(transparent)]
        AlreadyLocked(#[from] AlreadyLockedError),
        #[error(transparent)]
        Empty(#[from] EmptyError),
    }
}

#[cfg(test)]
mod tests {
    use near_sdk::AccountId;

    use super::WithdrawalQueue;

    // TODO: Test locking.

    #[test]
    fn withdrawal_remove() {
        let mut wq = WithdrawalQueue::new(b"w");

        let alice: AccountId = "alice".parse().unwrap();
        let bob: AccountId = "bob".parse().unwrap();
        let charlie: AccountId = "charlie".parse().unwrap();

        wq.insert_or_update(&alice, 1.into());
        wq.insert_or_update(&bob, 2.into());
        wq.insert_or_update(&charlie, 3.into());
        assert_eq!(wq.len(), 3);
        assert_eq!(wq.remove(&bob), Some(2.into()));
        assert_eq!(wq.len(), 2);
        assert_eq!(wq.remove(&charlie), Some(3.into()));
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.remove(&alice), Some(1.into()));
        assert_eq!(wq.len(), 0);
    }

    #[test]
    fn withdrawal_queueing() {
        let mut wq = WithdrawalQueue::new(b"w");

        let alice: AccountId = "alice".parse().unwrap();
        let bob: AccountId = "bob".parse().unwrap();
        let charlie: AccountId = "charlie".parse().unwrap();

        assert_eq!(wq.len(), 0);
        assert_eq!(wq.peek(), None);
        wq.insert_or_update(&alice, 1.into());
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.peek(), Some((alice.clone(), 1.into())));
        wq.insert_or_update(&alice, 99.into());
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.peek(), Some((alice.clone(), 99.into())));
        wq.insert_or_update(&bob, 123.into());
        assert_eq!(wq.len(), 2);
        wq.try_lock().unwrap();
        assert_eq!(wq.try_pop(), Some((alice.clone(), 99.into())));
        assert_eq!(wq.len(), 1);
        wq.insert_or_update(&charlie, 42.into());
        assert_eq!(wq.len(), 2);
        wq.try_lock().unwrap();
        assert_eq!(wq.try_pop(), Some((bob.clone(), 123.into())));
        assert_eq!(wq.len(), 1);
        wq.try_lock().unwrap();
        assert_eq!(wq.try_pop(), Some((charlie.clone(), 42.into())));
        assert_eq!(wq.len(), 0);
    }
}
