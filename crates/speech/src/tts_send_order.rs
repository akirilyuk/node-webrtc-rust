//! Call-order tickets for `VoiceAgent::send_text_to_tts*`.
//!
//! Each send takes a ticket synchronously when it is called. A send reaches the synthesis queue
//! only on its own turn, so the queue holds jobs in ticket (call) order even when a later send's
//! future is polled first. A ticket that never reaches the queue (error, cancel, dropped future)
//! releases its turn, so it never blocks later tickets.
//!
//! A flush or stop takes a cut: every ticket issued before the cut is cancelled, whether it is
//! already queued or still on its way to the queue.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

#[derive(Default)]
struct OrderState {
    /// Next ticket id to hand out.
    next_ticket: u64,
    /// Lowest ticket that has neither reached the queue nor released its turn.
    next_turn: u64,
    /// Tickets above `next_turn` that already released their turn (out of order).
    released_ahead: BTreeSet<u64>,
    /// Tickets below this id are cancelled (set by a flush / stop cut).
    cancel_below: u64,
}

/// Shared ticket sequencer (one per agent).
#[derive(Default)]
pub(crate) struct TtsSendOrder {
    state: Mutex<OrderState>,
    turn: Notify,
}

/// Why a ticket did not get its turn.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TicketCancelled;

/// A send's place in the synthesis queue order. Taken with
/// [`VoiceAgent::reserve_tts_send`](crate::VoiceAgent::reserve_tts_send) and consumed by
/// [`VoiceAgent::send_text_to_tts_reserved`](crate::VoiceAgent::send_text_to_tts_reserved).
///
/// Dropping an unused ticket releases its turn. Until it is sent or dropped, later tickets wait
/// for it, so do not hold one across unrelated work.
pub struct TtsSendTicket {
    id: u64,
    order: Arc<TtsSendOrder>,
    released: bool,
}

/// A flush / stop point taken with [`VoiceAgent::begin_flush_tts`](crate::VoiceAgent::begin_flush_tts):
/// tickets issued before it are cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TtsFlushCut(pub(crate) u64);

impl TtsSendTicket {
    /// Ticket id; ids start at 0 per agent and grow in call order.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Releases this ticket's turn now (after its job was queued, or when it gives up).
    pub(crate) fn release(&mut self) {
        if !self.released {
            self.released = true;
            self.order.release(self.id);
        }
    }
}

impl Drop for TtsSendTicket {
    fn drop(&mut self) {
        self.release();
    }
}

impl TtsSendOrder {
    fn lock(&self) -> std::sync::MutexGuard<'_, OrderState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes the next ticket. Synchronous: the ticket order is the call order.
    pub(crate) fn reserve(self: &Arc<Self>) -> TtsSendTicket {
        let id = {
            let mut s = self.lock();
            let id = s.next_ticket;
            s.next_ticket += 1;
            id
        };
        TtsSendTicket {
            id,
            order: Arc::clone(self),
            released: false,
        }
    }

    fn release(&self, id: u64) {
        {
            let mut s = self.lock();
            if id == s.next_turn {
                s.next_turn += 1;
                loop {
                    let next = s.next_turn;
                    if !s.released_ahead.remove(&next) {
                        break;
                    }
                    s.next_turn += 1;
                }
            } else if id > s.next_turn {
                s.released_ahead.insert(id);
            }
        }
        self.turn.notify_waiters();
    }

    /// Cancels every ticket issued so far (queued or still on its way). Synchronous.
    pub(crate) fn cut(&self) -> TtsFlushCut {
        let cut = {
            let mut s = self.lock();
            s.cancel_below = s.cancel_below.max(s.next_ticket);
            // Cancelled tickets can no longer be queued, so later tickets do not wait for them
            // to release. A cancelled ticket that already passed its turn and is queuing now
            // holds the queue lock, which keeps it ahead of the next ticket.
            if s.next_turn < s.cancel_below {
                s.next_turn = s.cancel_below;
                let cancel_below = s.cancel_below;
                let keep = s.released_ahead.split_off(&cancel_below);
                s.released_ahead = keep;
                loop {
                    let next = s.next_turn;
                    if !s.released_ahead.remove(&next) {
                        break;
                    }
                    s.next_turn += 1;
                }
            }
            s.next_ticket
        };
        self.turn.notify_waiters();
        TtsFlushCut(cut)
    }

    pub(crate) fn is_cancelled(&self, id: u64) -> bool {
        id < self.lock().cancel_below
    }

    /// Waits until every lower ticket has reached the queue or released its turn.
    /// Returns [`TicketCancelled`] as soon as a cut covers `id`. Each wake is driven by a lower
    /// ticket's release or by a cut, so the wait lasts only as long as the lower sends.
    pub(crate) async fn wait_turn(&self, id: u64) -> Result<(), TicketCancelled> {
        loop {
            let notified = self.turn.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let s = self.lock();
                if id < s.cancel_below {
                    return Err(TicketCancelled);
                }
                if s.next_turn == id {
                    return Ok(());
                }
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn released_out_of_order_advances_turn() {
        let order = Arc::new(TtsSendOrder::default());
        let a = order.reserve();
        let b = order.reserve();
        let mut c = order.reserve();
        drop(b);
        assert_eq!(order.lock().next_turn, 0);
        drop(a);
        assert_eq!(order.lock().next_turn, 2);
        order.wait_turn(c.id()).await.unwrap();
        c.release();
        assert_eq!(order.lock().next_turn, 3);
    }

    #[tokio::test]
    async fn cut_cancels_issued_tickets_only() {
        let order = Arc::new(TtsSendOrder::default());
        let _a = order.reserve();
        let b = order.reserve();
        let cut = order.cut();
        let c = order.reserve();
        assert_eq!(cut, TtsFlushCut(2));
        assert_eq!(order.wait_turn(b.id()).await, Err(TicketCancelled));
        assert!(order.is_cancelled(b.id()));
        assert!(!order.is_cancelled(c.id()));
        // `_a` and `b` are still held, yet the first ticket after the cut has its turn.
        order.wait_turn(c.id()).await.unwrap();
    }
}
