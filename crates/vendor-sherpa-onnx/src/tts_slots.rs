//! TTS synthesis slots shared by every session in the process, with two priorities.
//!
//! A reply's first sentence ([`TtsPriority::First`]) is served before later sentences of
//! replies that are already playing ([`TtsPriority::Continuation`]): a reply that is playing
//! has seconds of audio buffered, so its next sentence is not urgent, while a caller waits on
//! a new reply's first sentence. Order is FIFO within a priority.
//!
//! Continuation waiters are never starved: after [`MAX_FIRST_IN_A_ROW`] consecutive `First`
//! grants while a `Continuation` waits, the next free slot goes to the oldest `Continuation`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::oneshot;

/// Consecutive `First` grants allowed while a `Continuation` is waiting.
const MAX_FIRST_IN_A_ROW: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TtsPriority {
    /// First sentence of a reply: a caller is waiting for audio to start.
    First,
    /// Any later sentence of a reply: earlier audio is still buffered.
    Continuation,
}

struct State {
    /// Slots that are free right now.
    free: usize,
    first: VecDeque<oneshot::Sender<TtsSlotPermit>>,
    continuation: VecDeque<oneshot::Sender<TtsSlotPermit>>,
    /// `First` grants handed out in a row while a `Continuation` was waiting.
    first_streak: usize,
}

impl State {
    /// Drop waiters whose future was dropped, from the front of both queues.
    fn prune_closed_front(&mut self) {
        for queue in [&mut self.first, &mut self.continuation] {
            while queue.front().is_some_and(oneshot::Sender::is_closed) {
                queue.pop_front();
            }
        }
    }

    /// Choose the next waiter. Decided entirely here, under the lock.
    fn pick(&mut self) -> Option<oneshot::Sender<TtsSlotPermit>> {
        self.prune_closed_front();
        let continuation_waiting = !self.continuation.is_empty();
        let serve_continuation = continuation_waiting
            && (self.first.is_empty() || self.first_streak >= MAX_FIRST_IN_A_ROW);
        if serve_continuation {
            self.first_streak = 0;
            return self.continuation.pop_front();
        }
        let next = self.first.pop_front();
        if next.is_some() {
            if continuation_waiting {
                self.first_streak += 1;
            } else {
                self.first_streak = 0;
            }
        }
        next
    }
}

/// Process-wide TTS slots. See the module docs.
pub(crate) struct TtsSlots {
    state: Mutex<State>,
}

impl TtsSlots {
    pub(crate) fn new(slots: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                free: slots,
                first: VecDeque::new(),
                continuation: VecDeque::new(),
                first_streak: 0,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Wait for a slot. Cancel-safe: dropping the future before it resolves never leaks a
    /// slot, including when a grant raced the drop (the permit comes back through `Drop`).
    pub(crate) async fn acquire(self: &Arc<Self>, priority: TtsPriority) -> TtsSlotPermit {
        loop {
            let receiver = {
                let mut state = self.lock();
                if state.free > 0 {
                    state.free -= 1;
                    return TtsSlotPermit {
                        slots: Some(Arc::clone(self)),
                    };
                }
                let (sender, receiver) = oneshot::channel();
                match priority {
                    TtsPriority::First => state.first.push_back(sender),
                    TtsPriority::Continuation => state.continuation.push_back(sender),
                }
                receiver
            };
            // The lock is released before awaiting. A sender is only dropped unsent when the
            // slots themselves are gone, which `self` rules out; retry rather than panic.
            if let Ok(permit) = receiver.await {
                return permit;
            }
        }
    }

    /// Slots free right now.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn available(&self) -> usize {
        self.lock().free
    }

    /// `(First, Continuation)` waiters still waiting.
    #[cfg(test)]
    fn waiting(&self) -> (usize, usize) {
        let state = self.lock();
        let live = |queue: &VecDeque<oneshot::Sender<TtsSlotPermit>>| {
            queue.iter().filter(|sender| !sender.is_closed()).count()
        };
        (live(&state.first), live(&state.continuation))
    }

    /// Return a slot: hand it to the next waiter, or mark it free.
    fn release(self: &Arc<Self>) {
        loop {
            let next = {
                let mut state = self.lock();
                match state.pick() {
                    Some(sender) => sender,
                    None => {
                        state.free += 1;
                        return;
                    }
                }
            };
            // Sent outside the lock: a failed send hands the permit back, and dropping it
            // here would re-enter `release` while the lock is held.
            if self.deliver(next) {
                return;
            }
        }
    }

    /// Give the slot to `waiter`. `false` when its receiver is gone; the slot stays with the
    /// caller, who must pick another waiter or free it.
    fn deliver(self: &Arc<Self>, waiter: oneshot::Sender<TtsSlotPermit>) -> bool {
        let permit = TtsSlotPermit {
            slots: Some(Arc::clone(self)),
        };
        match waiter.send(permit) {
            Ok(()) => true,
            Err(mut permit) => {
                permit.slots = None;
                false
            }
        }
    }
}

/// One held slot. Dropping it returns the slot to the next waiter.
pub(crate) struct TtsSlotPermit {
    slots: Option<Arc<TtsSlots>>,
}

impl Drop for TtsSlotPermit {
    fn drop(&mut self) {
        if let Some(slots) = self.slots.take() {
            slots.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;

    /// Let spawned tasks run until the waiter counts match (no sleeps).
    async fn wait_for_waiting(slots: &Arc<TtsSlots>, expected: (usize, usize)) {
        while slots.waiting() != expected {
            tokio::task::yield_now().await;
        }
    }

    /// Spawn a task that acquires, records `name` on grant and releases at once.
    fn spawn_waiter(
        slots: &Arc<TtsSlots>,
        priority: TtsPriority,
        name: &'static str,
        grants: &mpsc::UnboundedSender<&'static str>,
    ) -> JoinHandle<()> {
        let slots = Arc::clone(slots);
        let grants = grants.clone();
        tokio::spawn(async move {
            let permit = slots.acquire(priority).await;
            let _ = grants.send(name);
            drop(permit);
        })
    }

    async fn collect(
        grants: &mut mpsc::UnboundedReceiver<&'static str>,
        count: usize,
    ) -> Vec<&'static str> {
        let mut out = Vec::new();
        for _ in 0..count {
            out.push(grants.recv().await.expect("grant"));
        }
        out
    }

    #[tokio::test]
    async fn first_sentence_overtakes_waiting_continuations() {
        let slots = TtsSlots::new(1);
        let holder = slots.acquire(TtsPriority::Continuation).await;
        let (tx, mut rx) = mpsc::unbounded_channel();

        spawn_waiter(&slots, TtsPriority::Continuation, "C1", &tx);
        wait_for_waiting(&slots, (0, 1)).await;
        spawn_waiter(&slots, TtsPriority::Continuation, "C2", &tx);
        wait_for_waiting(&slots, (0, 2)).await;
        spawn_waiter(&slots, TtsPriority::First, "F1", &tx);
        wait_for_waiting(&slots, (1, 2)).await;

        drop(holder);
        assert_eq!(collect(&mut rx, 3).await, vec!["F1", "C1", "C2"]);
    }

    #[tokio::test]
    async fn fifo_within_priority() {
        let slots = TtsSlots::new(1);
        let holder = slots.acquire(TtsPriority::First).await;
        let (tx, mut rx) = mpsc::unbounded_channel();

        spawn_waiter(&slots, TtsPriority::First, "F1", &tx);
        wait_for_waiting(&slots, (1, 0)).await;
        spawn_waiter(&slots, TtsPriority::First, "F2", &tx);
        wait_for_waiting(&slots, (2, 0)).await;
        spawn_waiter(&slots, TtsPriority::First, "F3", &tx);
        wait_for_waiting(&slots, (3, 0)).await;

        drop(holder);
        assert_eq!(collect(&mut rx, 3).await, vec!["F1", "F2", "F3"]);
    }

    #[tokio::test]
    async fn continuations_not_starved() {
        let slots = TtsSlots::new(1);
        let holder = slots.acquire(TtsPriority::First).await;
        let (tx, mut rx) = mpsc::unbounded_channel();

        spawn_waiter(&slots, TtsPriority::Continuation, "C", &tx);
        wait_for_waiting(&slots, (0, 1)).await;
        let names = ["F1", "F2", "F3", "F4", "F5"];
        for (index, name) in names.iter().enumerate() {
            spawn_waiter(&slots, TtsPriority::First, name, &tx);
            wait_for_waiting(&slots, (index + 1, 1)).await;
        }

        drop(holder);
        let order = collect(&mut rx, names.len() + 1).await;
        let position = order
            .iter()
            .position(|name| *name == "C")
            .expect("C granted");
        assert_eq!(position, MAX_FIRST_IN_A_ROW, "grant order: {order:?}");
        assert_eq!(order, vec!["F1", "F2", "F3", "C", "F4", "F5"]);
    }

    #[tokio::test]
    async fn dropped_waiter_does_not_leak_slot() {
        let slots = TtsSlots::new(1);
        let holder = slots.acquire(TtsPriority::First).await;
        let (tx, mut rx) = mpsc::unbounded_channel();

        let cancelled = spawn_waiter(&slots, TtsPriority::First, "gone", &tx);
        wait_for_waiting(&slots, (1, 0)).await;
        spawn_waiter(&slots, TtsPriority::Continuation, "kept", &tx);
        wait_for_waiting(&slots, (1, 1)).await;
        cancelled.abort();
        assert!(cancelled.await.is_err());
        wait_for_waiting(&slots, (0, 1)).await;

        // The cancelled waiter is skipped; the live one gets the slot and returns it.
        drop(holder);
        assert_eq!(collect(&mut rx, 1).await, vec!["kept"]);
        while slots.available() != 1 {
            tokio::task::yield_now().await;
        }

        // Safety net only; the acquire resolves immediately when no slot leaked.
        let permit = tokio::time::timeout(
            Duration::from_secs(30),
            slots.acquire(TtsPriority::Continuation),
        )
        .await
        .expect("slot leaked: acquire did not resolve");
        drop(permit);
        assert_eq!(slots.available(), 1);
    }

    #[tokio::test]
    async fn only_dropped_waiter_returns_slot_to_pool() {
        let slots = TtsSlots::new(1);
        let holder = slots.acquire(TtsPriority::First).await;
        let (tx, _rx) = mpsc::unbounded_channel();

        let cancelled = spawn_waiter(&slots, TtsPriority::First, "gone", &tx);
        wait_for_waiting(&slots, (1, 0)).await;
        cancelled.abort();
        assert!(cancelled.await.is_err());

        drop(holder);
        assert_eq!(slots.available(), 1);
    }

    #[tokio::test]
    async fn deliver_to_dropped_receiver_keeps_slot_with_caller() {
        let slots = TtsSlots::new(1);
        let holder = slots.acquire(TtsPriority::First).await;
        assert_eq!(slots.available(), 0);

        let (sender, receiver) = oneshot::channel();
        drop(receiver);
        assert!(!slots.deliver(sender), "send to a closed waiter must fail");
        // The failed send must not release the slot on its own.
        assert_eq!(slots.available(), 0);

        drop(holder);
        assert_eq!(slots.available(), 1);
    }

    #[tokio::test]
    async fn grant_race_with_drop_returns_permit() {
        let slots = TtsSlots::new(1);
        let mut holder = slots.acquire(TtsPriority::First).await;
        // Detach the permit so the slot is "in transit", owned by no permit.
        holder.slots = None;
        drop(holder);
        assert_eq!(slots.available(), 0);

        // The grant lands in the channel, then the waiter goes away before reading it.
        let (sender, receiver) = oneshot::channel();
        assert!(slots.deliver(sender));
        assert_eq!(slots.available(), 0);
        drop(receiver);
        assert_eq!(slots.available(), 1);
    }
}
