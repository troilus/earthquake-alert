use std::time::Duration;

pub(super) const HEALTHY_CONNECTION_UPTIME: Duration = Duration::from_secs(30);

pub(super) fn reset_after_healthy_uptime(
    delay: &mut Duration,
    reconnect_min: Duration,
    connected_for: Duration,
) {
    if connected_for >= HEALTHY_CONNECTION_UPTIME {
        *delay = reconnect_min;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resets_only_after_sustained_connection_uptime() {
        let minimum = Duration::from_secs(1);
        let mut delay = Duration::from_secs(16);
        reset_after_healthy_uptime(
            &mut delay,
            minimum,
            HEALTHY_CONNECTION_UPTIME.saturating_sub(Duration::from_millis(1)),
        );
        assert_eq!(delay, Duration::from_secs(16));

        reset_after_healthy_uptime(&mut delay, minimum, HEALTHY_CONNECTION_UPTIME);
        assert_eq!(delay, minimum);
    }
}
