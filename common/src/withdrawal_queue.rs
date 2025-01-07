use std::num::NonZeroU32;

use near_sdk::{collections::LookupMap, env, near, AccountId, BorshStorageKey, IntoStorageKey};

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct QueueNode {
    account_id: AccountId,
    amount: u128,
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

    pub fn len(&self) -> u32 {
        self.length
    }

    pub fn get(&self, account_id: &AccountId) -> Option<u128> {
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

    pub fn peek(&self) -> Option<(AccountId, u128)> {
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

    pub fn try_lock(&mut self) -> Option<(AccountId, u128)> {
        if self.is_locked {
            return None;
        }

        if let Some(peek) = self.peek() {
            self.is_locked = true;
            Some(peek)
        } else {
            None
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
    pub fn try_pop(&mut self) -> Option<(AccountId, u128)> {
        if !self.is_locked {
            env::panic_str("Withdrawal queue is locked.");
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
    pub fn remove(&mut self, account_id: &AccountId) -> Option<u128> {
        if self.is_locked && self.queue_head == self.entries.get(account_id) {
            env::panic_str("Withdrawal queue is locked.");
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

    pub fn insert_or_update(&mut self, account_id: &AccountId, amount: u128) {
        if let Some(node_id) = self.entries.get(account_id) {
            // update existing
            self.mut_existing_node(node_id, |node| node.amount = amount);
        } else {
            // add new
            let node_id = self.next_queue_node_id;
            self.next_queue_node_id = self.next_queue_node_id.checked_add(1).unwrap(); // assume the collection never processes more than u32::MAX items
            if let Some(tail_id) = self.queue_tail {
                self.mut_existing_node(tail_id, |tail| tail.next = Some(node_id));
            }
            let node = QueueNode {
                account_id: account_id.clone(),
                amount,
                prev: self.queue_tail,
                next: None,
            };
            if self.queue_head == None {
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
            withdrawal_queue: &self,
            next_node_id: self.queue_head,
        }
    }
}

pub struct WithdrawalQueueIter<'a> {
    withdrawal_queue: &'a WithdrawalQueue,
    next_node_id: Option<NonZeroU32>,
}

impl<'a> Iterator for WithdrawalQueueIter<'a> {
    type Item = (AccountId, u128);

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

        wq.insert_or_update(&alice, 1);
        wq.insert_or_update(&bob, 2);
        wq.insert_or_update(&charlie, 3);
        assert_eq!(wq.len(), 3);
        assert_eq!(wq.remove(&bob), Some(2));
        assert_eq!(wq.len(), 2);
        assert_eq!(wq.remove(&charlie), Some(3));
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.remove(&alice), Some(1));
        assert_eq!(wq.len(), 0);
    }

    #[test]
    fn withdrawal_queue() {
        let mut wq = WithdrawalQueue::new(b"w");

        let alice: AccountId = "alice".parse().unwrap();
        let bob: AccountId = "bob".parse().unwrap();
        let charlie: AccountId = "charlie".parse().unwrap();

        assert_eq!(wq.len(), 0);
        assert_eq!(wq.peek(), None);
        wq.insert_or_update(&alice, 1);
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.peek(), Some((alice.clone(), 1)));
        wq.insert_or_update(&alice, 99);
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.peek(), Some((alice.clone(), 99)));
        wq.insert_or_update(&bob, 123);
        assert_eq!(wq.len(), 2);
        assert_eq!(wq.try_pop(), Some((alice.clone(), 99)));
        assert_eq!(wq.len(), 1);
        wq.insert_or_update(&charlie, 42);
        assert_eq!(wq.len(), 2);
        assert_eq!(wq.try_pop(), Some((bob.clone(), 123)));
        assert_eq!(wq.len(), 1);
        assert_eq!(wq.try_pop(), Some((charlie.clone(), 42)));
        assert_eq!(wq.len(), 0);
    }
}
