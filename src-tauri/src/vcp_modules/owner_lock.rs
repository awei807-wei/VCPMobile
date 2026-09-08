use dashmap::DashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{Mutex, MutexGuard};

// The high bit marks an entry as closed.  A closed entry can no longer be
// acquired, which lets the last holder remove it without racing a new holder
// that already observed the old map entry.
const CLOSED_BIT: usize = 1usize << (usize::BITS - 1);

struct Entry {
    mutex: Mutex<()>,
    users: AtomicUsize,
}

impl Entry {
    fn new() -> Self {
        Self {
            mutex: Mutex::new(()),
            users: AtomicUsize::new(0),
        }
    }

    fn try_acquire(&self) -> bool {
        let mut current = self.users.load(Ordering::Acquire);
        loop {
            if current & CLOSED_BIT != 0 {
                return false;
            }
            let next = current
                .checked_add(1)
                .filter(|value| value & CLOSED_BIT == 0)
                .expect("owner lock user count overflow");
            match self.users.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn release(&self) -> bool {
        let mut current = self.users.load(Ordering::Acquire);
        loop {
            debug_assert!(current > 0 && current & CLOSED_BIT == 0);
            let next = current - 1;
            match self.users.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) if next == 0 => {
                    return self
                        .users
                        .compare_exchange(0, CLOSED_BIT, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok();
                }
                Ok(_) => return false,
                Err(observed) => current = observed,
            }
        }
    }
}

pub(crate) struct OwnerLockRegistry {
    entries: DashMap<String, Arc<Entry>>,
}

impl OwnerLockRegistry {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: DashMap::new(),
        })
    }

    pub(crate) fn acquire(self: &Arc<Self>, key: &str) -> OwnerLockHandle {
        loop {
            let entry = self
                .entries
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(Entry::new()))
                .value()
                .clone();
            if entry.try_acquire() {
                return OwnerLockHandle {
                    registry: Arc::clone(self),
                    key: key.to_string(),
                    entry,
                };
            }
            self.entries
                .remove_if(key, |_key, current| Arc::ptr_eq(current, &entry));
        }
    }

    /// Acquire a lock in an owner-type namespace. Agent and group ids are
    /// allowed to overlap, but their writes must never serialize through or
    /// accidentally share the same registry entry.
    pub(crate) fn acquire_owner(
        self: &Arc<Self>,
        owner_type: &str,
        owner_id: &str,
    ) -> OwnerLockHandle {
        self.acquire(&format!("{owner_type}\0{owner_id}"))
    }

    fn release(&self, key: &str, entry: &Arc<Entry>) {
        if entry.release() {
            self.entries
                .remove_if(key, |_key, current| Arc::ptr_eq(current, entry));
        }
    }
}

pub(crate) struct OwnerLockHandle {
    registry: Arc<OwnerLockRegistry>,
    key: String,
    entry: Arc<Entry>,
}

impl OwnerLockHandle {
    pub(crate) async fn lock(&self) -> MutexGuard<'_, ()> {
        self.entry.mutex.lock().await
    }

    /// The write queue runs its SQLite transaction inside `spawn_blocking`.
    /// Acquiring the same tokio mutex before opening that transaction prevents
    /// a local service writer from waiting on SQLite while the queue waits on
    /// its owner lock.
    pub(crate) fn blocking_lock(&self) -> MutexGuard<'_, ()> {
        self.entry.mutex.blocking_lock()
    }
}

impl Drop for OwnerLockHandle {
    fn drop(&mut self) {
        self.registry.release(&self.key, &self.entry);
    }
}

#[cfg(test)]
mod tests {
    use super::OwnerLockRegistry;
    use std::sync::Arc;

    #[tokio::test]
    async fn registry_releases_unused_owner_entries() {
        let registry = OwnerLockRegistry::new();
        let first = registry.acquire("missing-owner");
        let first_guard = first.lock().await;
        drop(first_guard);
        drop(first);
        assert!(registry.entries.is_empty());
    }

    #[tokio::test]
    async fn registry_keeps_waiters_on_the_same_entry_until_release() {
        let registry = OwnerLockRegistry::new();
        let first = registry.acquire("owner");
        let first_guard = first.lock().await;
        let waiting_registry = Arc::clone(&registry);
        let waiter = tokio::spawn(async move {
            let handle = waiting_registry.acquire("owner");
            let _guard = handle.lock().await;
        });
        tokio::task::yield_now().await;
        assert_eq!(registry.entries.len(), 1);
        drop(first_guard);
        drop(first);
        waiter.await.unwrap();
        assert!(registry.entries.is_empty());
    }

    #[tokio::test]
    async fn owner_type_namespaces_allow_same_id_without_cross_blocking() {
        let registry = OwnerLockRegistry::new();
        let agent = registry.acquire_owner("agent", "shared-id");
        let group = registry.acquire_owner("group", "shared-id");
        let _agent_guard = agent.lock().await;
        let _group_guard = group.lock().await;
        assert_eq!(registry.entries.len(), 2);
    }
}
