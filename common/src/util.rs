use std::ops::Deref;

use near_sdk::near;

#[derive(Clone, Debug)]
#[near]
pub struct Lockable<T> {
    inner: T,
    is_locked: bool,
}

impl<T> Deref for Lockable<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Lockable<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            is_locked: false,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.is_locked
    }

    pub fn lock(&mut self) {
        self.is_locked = true;
    }

    pub fn unlock(&mut self) {
        self.is_locked = false;
    }

    pub fn get(&self) -> &T {
        &self.inner
    }

    pub fn try_get_mut(&mut self) -> Option<&mut T> {
        if self.is_locked {
            None
        } else {
            Some(&mut self.inner)
        }
    }
}
