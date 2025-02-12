use std::ops::Deref;

use near_sdk::near;

#[derive(Clone, Debug)]
#[near]
pub enum Lockable<T> {
    Unlocked(T),
    Locked(T),
}

impl<T> Deref for Lockable<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl<T> Lockable<T> {
    pub fn is_locked(&self) -> bool {
        matches!(self, Self::Locked(..))
    }

    pub fn lock(self) -> Self {
        match self {
            Self::Unlocked(i) => Self::Locked(i),
            _ => self,
        }
    }

    pub fn unlock(self) -> Self {
        match self {
            Self::Locked(i) => Self::Unlocked(i),
            _ => self,
        }
    }

    pub fn to_unlocked(self) -> Option<T> {
        match self {
            Self::Unlocked(i) => Some(i),
            _ => None,
        }
    }

    pub fn get(&self) -> &T {
        match self {
            Self::Locked(ref i) | Self::Unlocked(ref i) => i,
        }
    }

    pub fn take(self) -> T {
        match self {
            Self::Locked(i) | Self::Unlocked(i) => i,
        }
    }

    pub fn try_get_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Unlocked(ref mut i) => Some(i),
            _ => None,
        }
    }
}
