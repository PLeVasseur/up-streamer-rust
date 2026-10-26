// SPDX-License-Identifier: Apache-2.0
//! Bounded application receive queues retaining the supplied message or storage lease.
//!
//! Transport listeners use nonblocking admission; consumers retain a delivery while
//! applying their configured processing delay. No payload copy, transport-specific
//! capacity allocation or middleware shutdown is performed by this queue.
use anyhow::{ensure, Result};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Default)]
struct Counters {
    received: AtomicU64,
    processed: AtomicU64,
    overload_drops: AtomicU64,
    closed_rejections: AtomicU64,
    abandoned: AtomicU64,
    live: AtomicU64,
    peak: AtomicU64,
}

/// Application queue/handle counters, separate from middleware-internal resources.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Snapshot {
    /// Configured maximum queued values.
    pub capacity: usize,
    /// Current queued values; an actively processed delivery is outside the queue.
    pub queued: usize,
    /// Values presented for nonblocking admission, including explicit rejections.
    pub received: u64,
    /// Accepted deliveries explicitly completed by the consumer.
    pub processed: u64,
    /// Admissions rejected because the bounded queue was full.
    pub overload_drops: u64,
    /// Admissions rejected after the receiver closed; these are not overload loss.
    pub closed_rejections: u64,
    /// Accepted deliveries dropped without an explicit completion.
    pub abandoned: u64,
    /// Values currently retained by queue, consumer or rejected-delivery handles.
    pub live_held: u64,
    /// Largest observed number of retained values in these handles.
    pub peak_held: u64,
}

/// Nonblocking listener-side admission into a bounded application inbox.
pub struct Sender<T> {
    inner: mpsc::Sender<Delivery<T>>,
    counters: Arc<Counters>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            counters: self.counters.clone(),
        }
    }
}

/// Sole consumer of admitted values, with explicit processing completion.
pub struct Receiver<T>(mpsc::Receiver<Delivery<T>>);

/// An owned message or storage lease retained independently of the queue's lifetime.
#[derive(Debug)]
pub struct Delivery<T> {
    value: Option<T>,
    counters: Arc<Counters>,
    accounted: bool,
}

impl<T> Delivery<T> {
    /// Borrows the retained value without copying it or transferring its storage owner.
    #[must_use]
    pub fn value(&self) -> &T {
        self.value
            .as_ref()
            .expect("delivery value retained until drop")
    }

    /// Records completed processing and releases this handle's retained value.
    ///
    /// A rejected delivery cannot be reclassified as processed. Dropping an accepted
    /// delivery without this call increments `abandoned`, exposing interrupted work.
    pub fn complete(mut self) -> Result<()> {
        ensure!(!self.accounted, "rejected delivery cannot be completed");
        self.accounted = true;
        self.counters.processed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl<T> Drop for Delivery<T> {
    fn drop(&mut self) {
        // Release the actual value before reporting that its handle no longer holds it.
        drop(self.value.take());
        if !self.accounted {
            self.counters.abandoned.fetch_add(1, Ordering::SeqCst);
        }
        let previous = self.counters.live.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(previous > 0, "retained-value count underflow");
    }
}

/// Creates a bounded inbox without constructing or configuring any transport.
///
/// Capacity is1..=65,536. Physical transport/loan capacity remains an independently
/// configured binding contract; this function cannot enlarge a middleware pool.
pub fn bounded<T>(capacity: usize) -> Result<(Sender<T>, Receiver<T>)> {
    ensure!(
        (1..=65_536).contains(&capacity),
        "inbox capacity outside bounded envelope"
    );
    let (sender, receiver) = mpsc::channel(capacity);
    Ok((
        Sender {
            inner: sender,
            counters: Arc::new(Counters::default()),
        },
        Receiver(receiver),
    ))
}

impl<T> Sender<T> {
    /// Attempts admission without waiting for a slow consumer.
    ///
    /// On failure the error retains the value so the listener can journal its exact
    /// identity before dropping it. Full and closed queues have distinct counters.
    /// Successful admission is not evidence of processing or end-to-end delivery.
    pub fn try_send(
        &self,
        value: T,
    ) -> std::result::Result<(), mpsc::error::TrySendError<Delivery<T>>> {
        self.try_send_with_admission(value, |_| {})
    }

    /// Attempts admission, invoking `admitted` only after a queue slot is reserved
    /// and before the value becomes visible to the consumer. This lets owners
    /// distinguish transient receive storage from admitted queued/processing leases.
    /// Rejections retain the original value and use the same explicit loss accounting.
    pub fn try_send_with_admission(
        &self,
        mut value: T,
        admitted: impl FnOnce(&mut T),
    ) -> std::result::Result<(), mpsc::error::TrySendError<Delivery<T>>> {
        self.counters.received.fetch_add(1, Ordering::SeqCst);
        let live = self.counters.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.counters.peak.fetch_max(live, Ordering::SeqCst);
        let permit = self.inner.try_reserve();
        if permit.is_ok() {
            admitted(&mut value);
        }
        let item = Delivery {
            value: Some(value),
            counters: self.counters.clone(),
            accounted: false,
        };
        let result = match permit {
            Ok(permit) => {
                permit.send(item);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(())) => Err(mpsc::error::TrySendError::Full(item)),
            Err(mpsc::error::TrySendError::Closed(())) => {
                Err(mpsc::error::TrySendError::Closed(item))
            }
        };
        result.map_err(|mut error| {
            match &mut error {
                mpsc::error::TrySendError::Full(item) => {
                    item.accounted = true;
                    self.counters.overload_drops.fetch_add(1, Ordering::SeqCst);
                }
                mpsc::error::TrySendError::Closed(item) => {
                    item.accounted = true;
                    self.counters
                        .closed_rejections
                        .fetch_add(1, Ordering::SeqCst);
                }
            }
            error
        })
    }

    /// Samples queue occupancy and handle counters. Final reconciliation requires quiescence.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let counters = &self.counters;
        Snapshot {
            capacity: self.inner.max_capacity(),
            queued: self.inner.max_capacity() - self.inner.capacity(),
            received: counters.received.load(Ordering::SeqCst),
            processed: counters.processed.load(Ordering::SeqCst),
            overload_drops: counters.overload_drops.load(Ordering::SeqCst),
            closed_rejections: counters.closed_rejections.load(Ordering::SeqCst),
            abandoned: counters.abandoned.load(Ordering::SeqCst),
            live_held: counters.live.load(Ordering::SeqCst),
            peak_held: counters.peak.load(Ordering::SeqCst),
        }
    }
}

impl<T> Receiver<T> {
    /// Waits for the next value, or returns `None` after all senders close and the queue drains.
    pub async fn recv(&mut self) -> Option<Delivery<T>> {
        self.0.recv().await
    }

    /// Receives an available value without waiting, distinguishing empty from disconnected.
    pub fn try_recv(&mut self) -> std::result::Result<Delivery<T>, mpsc::error::TryRecvError> {
        self.0.try_recv()
    }

    /// Stops new admissions while allowing already accepted values to drain.
    pub fn close(&mut self) {
        self.0.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admission_transition_precedes_visibility_and_never_runs_for_rejection() {
        let (sender, mut receiver) = bounded(1).unwrap();
        sender
            .try_send_with_admission(false, |admitted| {
                assert_eq!(sender.snapshot().queued, 1); // Reservation owns the slot.
                *admitted = true;
            })
            .unwrap();
        let rejected = sender
            .try_send_with_admission(false, |_| panic!("full queue cannot admit"))
            .unwrap_err();
        match rejected {
            mpsc::error::TrySendError::Full(delivery) => assert!(!delivery.value()),
            _ => panic!("expected full queue"),
        }
        let delivery = receiver.try_recv().unwrap();
        assert!(*delivery.value());
        delivery.complete().unwrap();
        receiver.close();
        assert!(sender
            .try_send_with_admission(false, |_| panic!("closed queue cannot admit"))
            .is_err());
        assert_eq!(sender.snapshot().live_held, 0);
    }
    #[derive(Debug)]
    struct Lease(Arc<AtomicU64>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn slow_consumer_has_bounded_admission_and_exact_retention_accounting() {
        let released = Arc::new(AtomicU64::new(0));
        let (sender, mut receiver) = bounded(2).unwrap();
        sender.try_send(Lease(released.clone())).unwrap();
        sender.try_send(Lease(released.clone())).unwrap();
        let processing = receiver.try_recv().unwrap();
        sender.try_send(Lease(released.clone())).unwrap();
        let rejected = sender.try_send(Lease(released.clone())).unwrap_err();
        assert!(matches!(rejected, mpsc::error::TrySendError::Full(_)));
        assert_eq!(sender.snapshot().queued, 2);
        assert_eq!(sender.snapshot().live_held, 4);
        assert_eq!(released.load(Ordering::SeqCst), 0);
        drop(rejected);
        processing.complete().unwrap();
        receiver.try_recv().unwrap().complete().unwrap();
        receiver.try_recv().unwrap().complete().unwrap();
        let final_state = sender.snapshot();
        assert_eq!(
            (
                final_state.received,
                final_state.processed,
                final_state.overload_drops
            ),
            (4, 3, 1)
        );
        assert_eq!(
            (
                final_state.queued,
                final_state.live_held,
                final_state.abandoned
            ),
            (0, 0, 0)
        );
        assert_eq!(released.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn retained_delivery_survives_receiver_shutdown_and_closed_is_not_overload() {
        let released = Arc::new(AtomicU64::new(0));
        let (sender, mut receiver) = bounded(1).unwrap();
        sender.try_send(Lease(released.clone())).unwrap();
        let retained = receiver.try_recv().unwrap();
        drop(receiver);
        assert!(Arc::ptr_eq(&retained.value().0, &released));
        assert_eq!(released.load(Ordering::SeqCst), 0);
        drop(sender.try_send(Lease(released.clone())).unwrap_err());
        retained.complete().unwrap();
        let final_state = sender.snapshot();
        assert_eq!(
            (
                final_state.processed,
                final_state.closed_rejections,
                final_state.overload_drops
            ),
            (1, 1, 0)
        );
        assert_eq!(final_state.live_held, 0);
        assert_eq!(released.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn abandoning_accepted_work_is_visible_instead_of_counted_as_processed() {
        let (sender, receiver) = bounded(1).unwrap();
        sender.try_send(1).unwrap();
        drop(receiver);
        let final_state = sender.snapshot();
        assert_eq!(
            (
                final_state.processed,
                final_state.abandoned,
                final_state.live_held
            ),
            (0, 1, 0)
        );
        assert!(bounded::<()>(0).is_err());
        assert!(bounded::<()>(65537).is_err());
    }
}
