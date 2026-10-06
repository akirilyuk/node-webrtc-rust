//! Keyed single-flight map: load each value once, without blocking other keys.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, TryLockError};

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

/// Per-key load slot. `None` until a load succeeds.
type Slot<V> = Arc<Mutex<Option<Arc<V>>>>;

/// Map of lazily loaded values with per-key single flight.
///
/// The map lock is held only to get or insert a slot; the load runs under the slot's own
/// lock. Concurrent first callers for one key therefore load once (the others wait on the
/// slot and then reuse the result), and callers for other keys never wait on that load.
/// A failed load leaves the slot empty, so the next caller retries.
///
/// Rules:
/// - Never hold the map lock while waiting on a slot lock: clone the slot `Arc`, drop the
///   map guard, then lock the slot.
/// - Readers that must not queue behind a load ([`get_loaded`](Self::get_loaded),
///   [`loaded_count`](Self::loaded_count), [`loaded_values`](Self::loaded_values)) use
///   `try_lock` on the slot; a slot that is mid-load is treated as not loaded.
/// - A poisoned lock maps to `SpeechError::Internal("sherpa <kind> pool lock poisoned")`
///   (readers that cannot return an error treat a poisoned slot as not loaded).
pub(crate) struct OnceMap<K, V> {
    kind: &'static str,
    map: Mutex<HashMap<K, Slot<V>>>,
}

impl<K: Eq + Hash + Clone, V> OnceMap<K, V> {
    pub(crate) fn new(kind: &'static str) -> Self {
        Self {
            kind,
            map: Mutex::new(HashMap::new()),
        }
    }

    fn poisoned(&self) -> SpeechError {
        SpeechError::Internal(format!("sherpa {} pool lock poisoned", self.kind))
    }

    /// Clone of every slot, taken under the map lock only (no slot lock is touched).
    fn slots(&self) -> Vec<Slot<V>> {
        match self.map.lock() {
            Ok(map) => map.values().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Returns `(value, created_now)`; `created_now` is true only for the caller whose
    /// `load` produced the value.
    pub(crate) fn get_or_load(
        &self,
        key: K,
        load: impl FnOnce() -> SpeechResult<V>,
    ) -> SpeechResult<(Arc<V>, bool)> {
        let slot = {
            let mut map = self.map.lock().map_err(|_| self.poisoned())?;
            Arc::clone(map.entry(key).or_insert_with(|| Arc::new(Mutex::new(None))))
        };
        // Map guard is dropped: a long load below blocks only callers of this key.
        let mut guard = slot.lock().map_err(|_| self.poisoned())?;
        if let Some(existing) = guard.as_ref() {
            return Ok((Arc::clone(existing), false));
        }
        let value = Arc::new(load()?);
        *guard = Some(Arc::clone(&value));
        Ok((value, true))
    }

    /// Loaded value without waiting for an in-flight load (`try_lock`; a load in progress
    /// yields `None`).
    pub(crate) fn get_loaded(&self, key: &K) -> Option<Arc<V>> {
        let slot = {
            let map = self.map.lock().ok()?;
            Arc::clone(map.get(key)?)
        };
        let loaded = match slot.try_lock() {
            Ok(guard) => guard.as_ref().map(Arc::clone),
            Err(TryLockError::WouldBlock) | Err(TryLockError::Poisoned(_)) => None,
        };
        loaded
    }

    /// Number of loaded values. A slot mid-load does not count.
    pub(crate) fn loaded_count(&self) -> usize {
        self.slots()
            .iter()
            .filter(|slot| match slot.try_lock() {
                Ok(guard) => guard.is_some(),
                Err(_) => false,
            })
            .count()
    }

    /// Snapshot of loaded values (same `try_lock` rule as [`loaded_count`](Self::loaded_count)).
    pub(crate) fn loaded_values(&self) -> Vec<Arc<V>> {
        self.slots()
            .iter()
            .filter_map(|slot| match slot.try_lock() {
                Ok(guard) => guard.as_ref().map(Arc::clone),
                Err(_) => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Barrier};
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(10);

    #[test]
    fn loads_once_for_concurrent_same_key() {
        const THREADS: usize = 8;
        let map: Arc<OnceMap<&'static str, usize>> = Arc::new(OnceMap::new("test"));
        let loads = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(THREADS));
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let (entered_tx, entered_rx) = mpsc::channel::<()>();

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let map = Arc::clone(&map);
                let loads = Arc::clone(&loads);
                let barrier = Arc::clone(&barrier);
                let release_rx = Arc::clone(&release_rx);
                let entered_tx = entered_tx.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let (value, created) = map
                        .get_or_load("a", || {
                            loads.fetch_add(1, Ordering::SeqCst);
                            let _ = entered_tx.send(());
                            release_rx
                                .lock()
                                .expect("release rx")
                                .recv_timeout(WAIT)
                                .expect("loader released");
                            Ok(42)
                        })
                        .expect("load");
                    (value, created)
                })
            })
            .collect();
        drop(entered_tx);

        // One loader parked inside the load; every other thread is past the barrier and
        // queued on the slot (or about to be). Release only after the loader is in.
        entered_rx.recv_timeout(WAIT).expect("a loader entered");
        release_tx.send(()).expect("release");

        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread"))
            .collect();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(results.iter().filter(|(_, created)| *created).count(), 1);
        for (value, _) in &results {
            assert!(Arc::ptr_eq(value, &results[0].0));
            assert_eq!(**value, 42);
        }
    }

    #[test]
    fn other_key_not_blocked_by_in_flight_load() {
        let map: Arc<OnceMap<&'static str, usize>> = Arc::new(OnceMap::new("test"));
        let (existing, created) = map.get_or_load("b", || Ok(1)).expect("load b");
        assert!(created);

        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let loader_map = Arc::clone(&map);
        let loading = std::thread::spawn(move || {
            loader_map
                .get_or_load("a", || {
                    let _ = entered_tx.send(());
                    release_rx.recv_timeout(WAIT).expect("loader released");
                    Ok(2)
                })
                .expect("load a")
        });
        entered_rx.recv_timeout(WAIT).expect("a is loading");

        // "a" is parked mid-load. A loaded key and a brand-new key must both proceed.
        let (again, created_again) = map
            .get_or_load("b", || -> SpeechResult<usize> { panic!("must not reload") })
            .expect("get b");
        assert!(!created_again);
        assert!(Arc::ptr_eq(&again, &existing));
        let (c, created_c) = map.get_or_load("c", || Ok(3)).expect("load c");
        assert!(created_c);
        assert_eq!(*c, 3);

        release_tx.send(()).expect("release");
        let (a, created_a) = loading.join().expect("loader thread");
        assert!(created_a);
        assert_eq!(*a, 2);
        assert!(map.get_loaded(&"a").is_some());
    }

    #[test]
    fn failed_load_is_not_cached() {
        let map: OnceMap<&'static str, usize> = OnceMap::new("test");
        let loads = AtomicUsize::new(0);

        let first = map.get_or_load("a", || {
            loads.fetch_add(1, Ordering::SeqCst);
            Err(SpeechError::Internal("boom".into()))
        });
        assert!(first.is_err());
        assert_eq!(map.loaded_count(), 0);

        let (value, created) = map
            .get_or_load("a", || {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok(7)
            })
            .expect("retry loads");
        assert!(created);
        assert_eq!(*value, 7);
        assert_eq!(loads.load(Ordering::SeqCst), 2);
        assert_eq!(map.loaded_count(), 1);
        assert!(map.get_loaded(&"a").is_some());
    }

    #[test]
    fn loaded_count_excludes_in_flight() {
        let map: Arc<OnceMap<&'static str, usize>> = Arc::new(OnceMap::new("test"));
        map.get_or_load("b", || Ok(1)).expect("load b");
        map.get_or_load("c", || Ok(2)).expect("load c");

        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let loader_map = Arc::clone(&map);
        let loading = std::thread::spawn(move || {
            loader_map
                .get_or_load("a", || {
                    let _ = entered_tx.send(());
                    release_rx.recv_timeout(WAIT).expect("loader released");
                    Ok(3)
                })
                .expect("load a");
        });
        entered_rx.recv_timeout(WAIT).expect("a is loading");

        // These return at once (try_lock), not after the load finishes.
        assert_eq!(map.loaded_count(), 2);
        assert!(map.get_loaded(&"a").is_none());
        assert_eq!(map.loaded_values().len(), 2);

        release_tx.send(()).expect("release");
        loading.join().expect("loader thread");
        assert_eq!(map.loaded_count(), 3);
        assert!(map.get_loaded(&"a").is_some());
    }
}
