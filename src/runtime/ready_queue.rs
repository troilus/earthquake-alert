use super::status::ReadyQueueMetrics;
use std::collections::VecDeque;
use std::mem;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

pub(super) struct ReadyQueue<T> {
    values: Mutex<VecDeque<T>>,
    wake: Notify,
    capacity_count: usize,
    capacity_bytes: usize,
    metrics: Arc<ReadyQueueMetrics>,
}

impl<T> ReadyQueue<T> {
    pub(super) fn new(
        capacity_count: usize,
        capacity_bytes: usize,
        metrics: Arc<ReadyQueueMetrics>,
    ) -> Self {
        Self {
            values: Mutex::new(VecDeque::with_capacity(capacity_count)),
            wake: Notify::new(),
            capacity_count,
            capacity_bytes,
            metrics,
        }
    }

    pub(super) fn try_push(&self, value: T) -> bool {
        let item_bytes = mem::size_of::<T>();
        let mut values = self
            .values
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let next_bytes = values.len().saturating_add(1).saturating_mul(item_bytes);
        if values.len() >= self.capacity_count || next_bytes > self.capacity_bytes {
            self.metrics.record_backpressure();
            return false;
        }
        values.push_back(value);
        self.metrics.set_depth(values.len(), next_bytes);
        drop(values);
        self.wake.notify_one();
        true
    }

    pub(super) fn pop(&self) -> Option<T> {
        let mut values = self
            .values
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let value = values.pop_front();
        self.metrics.set_depth(
            values.len(),
            values.len().saturating_mul(mem::size_of::<T>()),
        );
        value
    }

    pub(super) async fn notified(&self) {
        self.wake.notified().await;
    }

    pub(super) fn notify_waiters(&self) {
        self.wake.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_enforces_count_and_byte_bounds() {
        let metrics = Arc::new(ReadyQueueMetrics::default());
        let queue = ReadyQueue::new(4, 2 * mem::size_of::<u64>(), Arc::clone(&metrics));
        assert!(queue.try_push(1_u64));
        assert!(queue.try_push(2_u64));
        assert!(!queue.try_push(3_u64));
        assert_eq!(queue.pop(), Some(1_u64));
        assert!(queue.try_push(3_u64));
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.depth, 2);
        assert_eq!(snapshot.bytes, 2 * mem::size_of::<u64>());
        assert_eq!(snapshot.backpressure, 1);
    }
}
