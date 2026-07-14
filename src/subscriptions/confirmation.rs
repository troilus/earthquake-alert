use crate::delivery::{BarkNotifier, BarkPermit};
use crate::models::Subscription;
use crate::storage::try_now_millis;
use crate::subscriptions::{LeasedSubscriptionConfirmation, SubscriptionManager};
use anyhow::{Context, Result};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::Notify;

const CONFIRMATION_LEASE_MS: i64 = 60_000;
const MAX_CONFIRMATION_ATTEMPTS: u16 = 12;
const MAX_CONFIRMATION_AGE_MS: i64 = 24 * 60 * 60 * 1_000;
const IDLE_POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriptionConfirmationOutcome {
    Activated,
    Pending,
    Rejected,
    Superseded,
}

#[derive(Clone)]
pub(crate) struct SubscriptionConfirmationService {
    inner: Arc<ConfirmationInner>,
}

struct ConfirmationInner {
    store: SubscriptionManager,
    notifier: BarkNotifier,
    max_concurrent: usize,
    closing: AtomicBool,
    wake: Notify,
}

impl SubscriptionConfirmationService {
    pub(crate) fn new(
        store: SubscriptionManager,
        notifier: BarkNotifier,
        max_concurrent: usize,
    ) -> Self {
        Self {
            inner: Arc::new(ConfirmationInner {
                store,
                notifier,
                max_concurrent: max_concurrent.max(1),
                closing: AtomicBool::new(false),
                wake: Notify::new(),
            }),
        }
    }

    pub(crate) async fn begin(
        &self,
        subscription: Subscription,
    ) -> Result<LeasedSubscriptionConfirmation> {
        anyhow::ensure!(
            !self.inner.closing.load(Ordering::Acquire),
            "subscription confirmation service is closing"
        );
        let store = self.inner.store.clone();
        let leased = tokio::task::spawn_blocking(move || {
            store.begin_confirmation(subscription, try_now_millis()?, CONFIRMATION_LEASE_MS)
        })
        .await
        .context("subscription confirmation begin task failed")??;
        self.inner.wake.notify_one();
        Ok(leased)
    }

    pub(crate) async fn attempt(
        &self,
        leased: LeasedSubscriptionConfirmation,
    ) -> Result<SubscriptionConfirmationOutcome> {
        if confirmation_expired(leased.created_at_ms, try_now_millis()?) {
            return self.abandon_expired(leased).await;
        }
        let permit = self.inner.notifier.acquire_permit().await;
        let permit = match permit {
            Ok(permit) => permit,
            Err(error) => return self.finish_failed_attempt(leased, error).await,
        };
        self.attempt_with_permit(leased, permit, true).await
    }

    async fn attempt_with_permit(
        &self,
        leased: LeasedSubscriptionConfirmation,
        permit: BarkPermit,
        renew_lease: bool,
    ) -> Result<SubscriptionConfirmationOutcome> {
        if confirmation_expired(leased.created_at_ms, try_now_millis()?) {
            return self.abandon_expired(leased).await;
        }
        let id = leased.id;
        let token = leased.lease_token;
        let store = self.inner.store.clone();
        if renew_lease {
            let renewed = tokio::task::spawn_blocking(move || {
                store.renew_confirmation_lease(id, token, try_now_millis()?, CONFIRMATION_LEASE_MS)
            })
            .await
            .context("subscription confirmation renewal task failed")??;
            if !renewed {
                return Ok(SubscriptionConfirmationOutcome::Superseded);
            }
        }
        match self
            .inner
            .notifier
            .send_subscription_confirm_with_permit(&leased.subscription, permit)
            .await
        {
            Ok(()) => {
                let store = self.inner.store.clone();
                let activated =
                    tokio::task::spawn_blocking(move || store.activate_confirmation(id, token))
                        .await
                        .context("subscription confirmation activation task failed")??;
                Ok(if activated {
                    SubscriptionConfirmationOutcome::Activated
                } else {
                    SubscriptionConfirmationOutcome::Superseded
                })
            }
            Err(error) => self.finish_failed_attempt(leased, error).await,
        }
    }

    async fn finish_failed_attempt(
        &self,
        leased: LeasedSubscriptionConfirmation,
        error: crate::delivery::BarkDeliveryError,
    ) -> Result<SubscriptionConfirmationOutcome> {
        let id = leased.id;
        let token = leased.lease_token;
        let attempted_at_ms = try_now_millis()?;
        let provider_permanent = error.is_permanent();
        let exhausted =
            confirmation_retry_exhausted(leased.attempts, leased.created_at_ms, attempted_at_ms);
        let message = format!("{error:#}");
        let store = self.inner.store.clone();
        if provider_permanent || exhausted {
            let abandoned =
                tokio::task::spawn_blocking(move || store.abandon_confirmation(id, token))
                    .await
                    .context("subscription confirmation abandon task failed")??;
            if abandoned {
                tracing::warn!(
                    event = "subscription.confirmation_rejected",
                    operation_id = id,
                    provider_permanent,
                    retry_exhausted = exhausted,
                    attempts = leased.attempts.saturating_add(1),
                    error = ?error,
                    "subscription.confirmation_rejected"
                );
                Ok(SubscriptionConfirmationOutcome::Rejected)
            } else {
                Ok(SubscriptionConfirmationOutcome::Superseded)
            }
        } else {
            let due_at_ms =
                attempted_at_ms.saturating_add(confirmation_retry_delay_ms(leased.attempts));
            let rescheduled = tokio::task::spawn_blocking(move || {
                store.reschedule_confirmation(id, token, attempted_at_ms, due_at_ms, &message)
            })
            .await
            .context("subscription confirmation reschedule task failed")??;
            if rescheduled {
                self.inner.wake.notify_one();
                Ok(SubscriptionConfirmationOutcome::Pending)
            } else {
                Ok(SubscriptionConfirmationOutcome::Superseded)
            }
        }
    }

    async fn abandon_expired(
        &self,
        leased: LeasedSubscriptionConfirmation,
    ) -> Result<SubscriptionConfirmationOutcome> {
        let id = leased.id;
        let token = leased.lease_token;
        let store = self.inner.store.clone();
        let abandoned = tokio::task::spawn_blocking(move || store.abandon_confirmation(id, token))
            .await
            .context("expired subscription confirmation abandon task failed")??;
        Ok(if abandoned {
            tracing::warn!(
                event = "subscription.confirmation_expired",
                operation_id = id,
                "subscription.confirmation_expired"
            );
            SubscriptionConfirmationOutcome::Rejected
        } else {
            SubscriptionConfirmationOutcome::Superseded
        })
    }

    pub(crate) fn close(&self) {
        self.inner.closing.store(true, Ordering::Release);
        self.inner.wake.notify_waiters();
    }

    pub(crate) async fn run(&self) -> Result<()> {
        let mut attempts = tokio::task::JoinSet::new();
        loop {
            while let Some(result) = attempts.try_join_next() {
                observe_attempt_result(result)?;
            }
            if self.inner.closing.load(Ordering::Acquire) {
                if attempts.is_empty() {
                    return Ok(());
                }
            } else {
                let available = self.inner.max_concurrent.saturating_sub(attempts.len());
                if available > 0 {
                    let mut permits = Vec::with_capacity(available);
                    for _ in 0..available {
                        let Some(permit) = self.inner.notifier.try_acquire_permit()? else {
                            break;
                        };
                        permits.push(permit);
                    }
                    if permits.is_empty() {
                        tokio::time::sleep(IDLE_POLL).await;
                        continue;
                    }
                    let store = self.inner.store.clone();
                    let limit = permits.len();
                    let leased = tokio::task::spawn_blocking(move || {
                        store.lease_due_confirmations(
                            try_now_millis()?,
                            CONFIRMATION_LEASE_MS,
                            MAX_CONFIRMATION_AGE_MS,
                            limit,
                        )
                    })
                    .await
                    .context("subscription confirmation lease task failed")??;
                    if !leased.is_empty() {
                        for (operation, permit) in leased.into_iter().zip(permits) {
                            let service = self.clone();
                            attempts.spawn(async move {
                                service.attempt_with_permit(operation, permit, false).await
                            });
                        }
                        continue;
                    }
                }
            }

            if attempts.is_empty() {
                tokio::select! {
                    () = tokio::time::sleep(IDLE_POLL) => {}
                    () = self.inner.wake.notified() => {}
                }
            } else {
                tokio::select! {
                    result = attempts.join_next() => {
                        if let Some(result) = result {
                            observe_attempt_result(result)?;
                        }
                    }
                    () = tokio::time::sleep(IDLE_POLL) => {}
                    () = self.inner.wake.notified() => {}
                }
            }
        }
    }
}

fn observe_attempt_result(
    result: std::result::Result<Result<SubscriptionConfirmationOutcome>, tokio::task::JoinError>,
) -> Result<()> {
    let outcome = result.context("subscription confirmation attempt task failed")??;
    tracing::debug!(
        event = "subscription.confirmation_attempt_completed",
        outcome = ?outcome,
        "subscription.confirmation_attempt_completed"
    );
    Ok(())
}

fn confirmation_retry_exhausted(attempts: u16, created_at_ms: i64, attempted_at_ms: i64) -> bool {
    attempts.saturating_add(1) >= MAX_CONFIRMATION_ATTEMPTS
        || attempted_at_ms.saturating_sub(created_at_ms) >= MAX_CONFIRMATION_AGE_MS
}

fn confirmation_expired(created_at_ms: i64, now_ms: i64) -> bool {
    now_ms.saturating_sub(created_at_ms) >= MAX_CONFIRMATION_AGE_MS
}

fn confirmation_retry_delay_ms(attempts: u16) -> i64 {
    let exponent = u32::from(attempts.min(10));
    1_000_i64
        .saturating_mul(1_i64 << exponent)
        .min(15 * 60 * 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::BarkPushConfig;
    use crate::models::{
        AlertRule, DisasterCategory, GeoPoint, MonitoringTarget, NotificationDestination,
    };
    use crate::storage::FjallStorage;

    #[test]
    fn confirmation_retry_budget_is_bounded_by_attempts_and_age() {
        assert!(!confirmation_retry_exhausted(10, 1_000, 2_000));
        assert!(confirmation_retry_exhausted(11, 1_000, 2_000));
        assert!(!confirmation_retry_exhausted(
            0,
            1_000,
            1_000 + MAX_CONFIRMATION_AGE_MS - 1
        ));
        assert!(confirmation_retry_exhausted(
            0,
            1_000,
            1_000 + MAX_CONFIRMATION_AGE_MS
        ));
    }

    #[tokio::test]
    async fn background_worker_acquires_bark_capacity_before_leasing() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let manager = SubscriptionManager::new(FjallStorage::open(directory.path())?);
        let notifier = BarkNotifier::new(
            vec!["https://api.day.app".to_string()],
            1,
            1,
            BarkPushConfig::new(None, 10, "test".to_string(), false),
        )?;
        let held_permit = notifier.acquire_permit().await?;
        let leased = manager.begin_confirmation(test_subscription(), 0, 1_000)?;
        let service = SubscriptionConfirmationService::new(manager.clone(), notifier, 1);
        let running_service = service.clone();
        let worker = tokio::spawn(async move { running_service.run().await });

        tokio::time::sleep(IDLE_POLL.saturating_mul(2)).await;
        service.close();
        worker.await.context("confirmation worker task failed")??;
        drop(held_permit);

        anyhow::ensure!(manager.activate_confirmation(leased.id, leased.lease_token)?);
        Ok(())
    }

    fn test_subscription() -> Subscription {
        Subscription::new(
            NotificationDestination::Bark {
                base_url: "https://api.day.app".to_string(),
                device_key: "device1".to_string(),
            },
            vec![MonitoringTarget {
                label: "home".to_string(),
                point: GeoPoint {
                    latitude: 35.0,
                    longitude: 105.0,
                },
                region: Default::default(),
            }],
            vec![AlertRule::default_for(DisasterCategory::EarthquakeReport)],
        )
    }
}
