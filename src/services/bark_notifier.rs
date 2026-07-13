use crate::db::{SubscriptionSnapshot, SubscriptionStore};
use crate::models::{
    AlertRule, DestinationId, DisasterCategory, DisasterEvent, ProviderChannel, SourceSelection,
    Subscription, mask_device_key,
};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use url::Url;

const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_TITLE_CHARS: usize = 180;
const MAX_SUBTITLE_CHARS: usize = 180;
const MAX_BODY_CHARS: usize = 4_000;
const MAX_AFFECTED_REGIONS: usize = 20;
const MAX_REGION_CHARS: usize = 80;

#[derive(Debug, Clone)]
pub struct BarkPushConfig {
    pub sound: Option<String>,
    pub volume: u8,
    pub group: String,
    pub call: bool,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AlertTiming {
    pub distance_km: f64,
    pub hypocentral_km: f64,
    pub estimated_intensity: f64,
    pub seconds_to_p: i64,
    pub seconds_to_s: i64,
}

#[derive(Debug, Clone)]
pub struct AlertRecipient {
    pub destination: DestinationId,
    pub location_name: String,
}

struct BarkMessage<'a> {
    bark_url: &'a str,
    device_key: &'a str,
    level: &'a str,
    title: &'a str,
    subtitle: &'a str,
    body: &'a str,
    use_alert_sound: bool,
}

/// Bark 推送客户端，负责受限并发的可靠投递。
#[derive(Clone)]
pub struct BarkNotifier {
    allowed_urls: Arc<HashSet<String>>,
    client: reqwest::Client,
    subscription_store: SubscriptionStore,
    push_config: BarkPushConfig,
    concurrency: Arc<Semaphore>,
}

impl BarkNotifier {
    pub fn new(
        allowed_urls: Vec<String>,
        pool_size: usize,
        max_concurrent: usize,
        subscription_store: SubscriptionStore,
        push_config: BarkPushConfig,
    ) -> Result<Self> {
        push_config.validate()?;
        anyhow::ensure!(
            !allowed_urls.is_empty(),
            "Bark URL allowlist cannot be empty"
        );
        let client = reqwest::Client::builder()
            .user_agent("DisasterAlert/1.0")
            .timeout(Duration::from_secs(3))
            .connect_timeout(Duration::from_secs(3))
            .pool_max_idle_per_host(pool_size)
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(60))
            .http2_adaptive_window(true)
            .http2_keep_alive_interval(Duration::from_secs(30))
            .http2_keep_alive_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        tracing::info!(
            event = "bark.initialized",
            allowed_url_count = allowed_urls.len(),
            pool_size,
            "bark.initialized"
        );
        Ok(Self {
            allowed_urls: Arc::new(allowed_urls.into_iter().collect()),
            client,
            subscription_store,
            push_config,
            concurrency: Arc::new(Semaphore::new(max_concurrent.max(1))),
        })
    }

    pub fn allows_bark_url(&self, bark_url: &str) -> bool {
        self.allowed_urls.contains(bark_url)
    }

    pub fn is_subscription_current(&self, subscription: &SubscriptionSnapshot) -> bool {
        self.subscription_store.is_current(subscription)
    }

    pub async fn send_disaster_alert(
        &self,
        recipient: &AlertRecipient,
        level: &str,
        event: &DisasterEvent,
        timing: Option<&AlertTiming>,
    ) -> Result<()> {
        let location = match (event.latitude, event.longitude) {
            (Some(latitude), Some(longitude)) => format!("{latitude:.2}, {longitude:.2}"),
            _ => "位置未知".to_string(),
        };
        let display_title = if event.cancel {
            format!("{}（解除/取消）", event.title)
        } else if event.final_report {
            format!("{}（终报）", event.title)
        } else {
            event.title.clone()
        };
        let subtitle = truncate_chars(
            &format!("{} · {}", event.category.as_str(), event.source),
            MAX_SUBTITLE_CHARS,
        );
        if event.category == DisasterCategory::EarthquakeWarning
            && let Some(timing) = timing
        {
            let (display_title, body) =
                earthquake_warning_message(recipient, level, event, timing, &location, &subtitle);
            return self
                .send_notification(BarkMessage {
                    bark_url: &recipient.destination.base_url,
                    device_key: &recipient.destination.device_key,
                    level,
                    title: &display_title,
                    subtitle: "",
                    body: &body,
                    use_alert_sound: true,
                })
                .await;
        }
        let mut lines = Vec::new();
        if event.training {
            lines.push("[测试] 这是一条模拟灾害信息".to_string());
        }
        if event.category == DisasterCategory::EarthquakeReport {
            if let Some(magnitude) = event.magnitude {
                lines.push(format!("震级: M{magnitude:.1}"));
            }
            match (event.latitude, event.longitude) {
                (Some(_), Some(_)) => lines.push(format!("震中: {location}")),
                _ => lines.push("震中: 位置未知".to_string()),
            }
            if let Some(depth_km) = event.depth_km {
                lines.push(format!("深度: {depth_km:.0}km"));
            }
        } else {
            lines.push(format!("位置: {location}"));
            if let Some(magnitude) = event.magnitude {
                lines.push(format!("震级: M{magnitude:.1}"));
            }
        }
        if let Some(timing) = timing {
            lines.push(format!(
                "距离: 震中{:.0}km 震源{:.0}km",
                timing.distance_km, timing.hypocentral_km
            ));
            lines.push(format!(
                "预计: P波{:+}秒 S波{:+}秒 烈度{:.1}",
                timing.seconds_to_p, timing.seconds_to_s, timing.estimated_intensity
            ));
        }
        if let Some(radius_km) = event.radius_km {
            lines.push(format!("七级风圈: {radius_km:.0}km"));
        }
        if !event.affected_regions.is_empty() {
            let regions = event
                .affected_regions
                .iter()
                .take(MAX_AFFECTED_REGIONS)
                .map(|region| truncate_chars(region, MAX_REGION_CHARS))
                .collect::<Vec<_>>();
            lines.push(format!("影响区域: {}", regions.join("、")));
        }
        if event.category != DisasterCategory::EarthquakeReport
            && !event.description.trim().is_empty()
        {
            lines.push(truncate_chars(&event.description, MAX_BODY_CHARS));
        }
        if !event.occurred_at.trim().is_empty() {
            lines.push(format!("时间: {}", event.occurred_at));
        }
        if !recipient.location_name.trim().is_empty() {
            lines.push(format!("监测点: {}", recipient.location_name.trim()));
        }
        lines.push(format!("来源: {} ({})", event.source, event.channel));
        lines.push(format!("副标题: {subtitle}"));
        let display_title = truncate_chars(&display_title, MAX_TITLE_CHARS);
        let body = truncate_chars(&lines.join("\n"), MAX_BODY_CHARS);
        self.send_notification(BarkMessage {
            bark_url: &recipient.destination.base_url,
            device_key: &recipient.destination.device_key,
            level,
            title: &display_title,
            subtitle: "",
            body: &body,
            use_alert_sound: true,
        })
        .await
    }

    pub async fn send_subscription_confirm(&self, subscription: &Subscription) -> Result<()> {
        let title = "灾害预警接收测试";
        let (subtitle, body) = subscription_confirmation_summary(subscription);

        self.send_notification(BarkMessage {
            bark_url: subscription.bark_base_url(),
            device_key: subscription.device_key(),
            level: "timeSensitive",
            title,
            subtitle: &subtitle,
            body: &body,
            use_alert_sound: false,
        })
        .await
    }

    pub async fn send_rule_test(&self, subscription: &Subscription) -> Result<()> {
        let alert = subscription
            .alerts
            .first()
            .context("test alert rule is missing")?;
        let target = subscription
            .targets
            .first()
            .context("test alert target is missing")?;
        let category = alert.category();
        let source = test_source(alert.sources());
        let (title, description, magnitude, radius_km, level, timing) = match alert {
            AlertRule::EarthquakeWarning {
                estimated_intensity_bands,
                ..
            } => {
                let band = estimated_intensity_bands
                    .iter()
                    .max_by_key(|band| band.max)
                    .context("test alert intensity band is missing")?;
                (
                    "模拟震源".to_string(),
                    format!("{} 测试", band.interruption_level.as_str()),
                    Some(6.2),
                    None,
                    band.interruption_level.as_str(),
                    Some(AlertTiming {
                        distance_km: 425.0,
                        hypocentral_km: 425.0,
                        estimated_intensity: f64::from(band.max),
                        seconds_to_p: 53,
                        seconds_to_s: 94,
                    }),
                )
            }
            AlertRule::EarthquakeReport { min_magnitude, .. } => (
                "[测试] 地震速报".to_string(),
                format!("当前规则测试：最低震级 M{min_magnitude:.1}"),
                Some(*min_magnitude),
                None,
                "active",
                None,
            ),
            AlertRule::WeatherWarning {
                min_severity,
                fallback_radius_km,
                ..
            } => (
                "[测试] 气象预警".to_string(),
                format!(
                    "当前规则测试：最低严重度 {min_severity}，坐标回退半径 {fallback_radius_km:.0}km"
                ),
                None,
                None,
                bark_level(*min_severity),
                None,
            ),
            AlertRule::Tsunami { min_severity, .. } => (
                "[测试] 海啸预警".to_string(),
                format!("当前规则测试：最低严重度 {min_severity}"),
                None,
                None,
                bark_level(*min_severity),
                None,
            ),
            AlertRule::Typhoon {
                max_center_distance_km,
                ..
            } => (
                "[测试] 台风信息".to_string(),
                format!("当前规则测试：中心距离 {max_center_distance_km:.0}km 内"),
                None,
                Some(180.0),
                "active",
                None,
            ),
        };
        let regions = [
            target.region.province.trim(),
            target.region.city.trim(),
            target.region.district.trim(),
        ]
        .into_iter()
        .filter(|region| !region.is_empty())
        .map(ToOwned::to_owned)
        .collect();
        let event = DisasterEvent {
            category,
            channel: ProviderChannel::FanStudio,
            source,
            event_id: "rule-test".to_string(),
            revision: "1".to_string(),
            report_num: 1,
            title,
            description,
            latitude: Some(target.point.latitude),
            longitude: Some(target.point.longitude),
            magnitude,
            depth_km: magnitude.map(|_| 10.0),
            affected_regions: regions,
            radius_km,
            level: 1,
            occurred_at: "当前规则测试".to_string(),
            final_report: false,
            cancel: false,
            training: true,
        };
        let recipient = AlertRecipient {
            destination: subscription.destination_id(),
            location_name: target.label.clone(),
        };
        self.send_disaster_alert(&recipient, level, &event, timing.as_ref())
            .await
    }

    async fn send_notification(&self, message: BarkMessage<'_>) -> Result<()> {
        let level = normalize_bark_level(message.level);
        let payload = bark_payload(&message, &self.push_config, level);
        let BarkMessage {
            bark_url,
            device_key,
            level: _,
            title: _,
            subtitle: _,
            body: _,
            use_alert_sound: _,
        } = message;
        anyhow::ensure!(
            self.allows_bark_url(bark_url),
            "订阅使用的 Bark URL 已被管理员停用，请重新配置"
        );
        let url = format!("{bark_url}/push");

        let mut retries = 0;
        let max_retries = 2;

        loop {
            let permit = self
                .concurrency
                .acquire()
                .await
                .context("Bark notification dispatcher closed")?;
            let response = self.client.post(&url).json(&payload).send().await;
            match response {
                Ok(response) => {
                    let status = response.status();
                    let status_code = status.as_u16();

                    if status.is_success() {
                        let body_text = limited_response_text(response).await?;
                        if bark_response_succeeded(&body_text) {
                            tracing::debug!(
                                event = "bark.push_succeeded",
                                device_key = %mask_device_key(device_key),
                                status = status_code,
                                "bark.push_succeeded"
                            );
                            drop(permit);
                            return Ok(());
                        }

                        tracing::warn!(
                            event = "bark.push_rejected",
                            device_key = %mask_device_key(device_key),
                            status = status_code,
                            cleanup = false,
                            "bark.push_rejected"
                        );
                        drop(permit);
                        return Err(anyhow::anyhow!("Bark 服务拒绝了推送"));
                    } else {
                        let _error_text = limited_response_text(response).await?;

                        if status.is_client_error() {
                            tracing::warn!(
                                event = "bark.push_rejected",
                                device_key = %mask_device_key(device_key),
                                status = status_code,
                                cleanup = false,
                                "bark.push_rejected"
                            );
                            drop(permit);
                            return Err(anyhow::anyhow!(
                                "Bark 服务拒绝了推送 (HTTP {})",
                                status_code
                            ));
                        }

                        if status.is_server_error() && retries < max_retries {
                            drop(permit);
                            retries += 1;
                            tracing::warn!(
                                event = "bark.push_retrying",
                                device_key = %mask_device_key(device_key),
                                retry = retries,
                                max_retries,
                                status = status.as_u16(),
                                "bark.push_retrying"
                            );
                            tokio::time::sleep(backoff_delay(retries)).await;
                            continue;
                        }

                        tracing::error!(
                            event = "bark.push_failed",
                            device_key = %mask_device_key(device_key),
                            status = status.as_u16(),
                            "bark.push_failed"
                        );
                        drop(permit);
                        return Err(anyhow::anyhow!("Bark 推送失败: {}", status));
                    }
                }
                Err(e) => {
                    if retries < max_retries {
                        drop(permit);
                        retries += 1;
                        tracing::warn!(
                            event = "bark.request_retrying",
                            device_key = %mask_device_key(device_key),
                            retry = retries,
                            max_retries,
                            error = ?e,
                            "bark.request_retrying"
                        );
                        tokio::time::sleep(backoff_delay(retries)).await;
                        continue;
                    }

                    tracing::error!(
                        event = "bark.request_failed",
                        device_key = %mask_device_key(device_key),
                        error = ?e,
                        "bark.request_failed"
                    );
                    drop(permit);
                    return Err(e.into());
                }
            }
        }
    }
}

fn test_source(sources: &SourceSelection) -> String {
    match sources {
        SourceSelection::All => "rule-test.all-sources".to_string(),
        SourceSelection::Include { ids } => ids
            .first()
            .cloned()
            .unwrap_or_else(|| "rule-test.selected-source".to_string()),
    }
}

fn earthquake_warning_message(
    recipient: &AlertRecipient,
    level: &str,
    event: &DisasterEvent,
    timing: &AlertTiming,
    location: &str,
    subtitle: &str,
) -> (String, String) {
    let magnitude = event.magnitude.unwrap_or_default();
    let place = event
        .title
        .trim()
        .strip_prefix("地震预警")
        .map(str::trim)
        .filter(|place| !place.is_empty())
        .unwrap_or(event.title.trim());
    let prefix = if event.training { "[测试] " } else { "" };
    let arrival = arrival_status(timing.seconds_to_s);
    let suffix = if event.cancel {
        "预警已取消".to_string()
    } else if event.final_report {
        format!("终报 · {arrival}")
    } else {
        arrival
    };
    let title = truncate_chars(
        &format!("{prefix}{place} M{magnitude:.1} · {suffix}"),
        MAX_TITLE_CHARS,
    );
    let mut lines = vec![
        format!(
            "预计烈度{:.1}（{}）",
            timing.estimated_intensity,
            intensity_feeling(timing.estimated_intensity)
        ),
        format!(
            "预计到达时间：{}",
            estimated_arrival_time(timing.seconds_to_s)
        ),
        String::new(),
    ];
    if event.training {
        lines.push("[测试] 这是一条模拟预警，不是真实地震。".to_string());
    }
    if event.training {
        lines.push(format!(
            "{place}（{} 测试） 距{:.0}km",
            level, timing.distance_km
        ));
    }
    if !event.occurred_at.trim().is_empty() {
        lines.push(format!("发震时间：{}", event.occurred_at));
    }
    if !recipient.location_name.trim().is_empty() {
        lines.push(format!("监测地点：{}", recipient.location_name.trim()));
    }
    lines.push(format!(
        "距离：震中{:.0}km · 震源{:.0}km",
        timing.distance_km, timing.hypocentral_km
    ));
    lines.push(format!(
        "预计：P波{} · S波{}",
        wave_status(timing.seconds_to_p),
        wave_status(timing.seconds_to_s)
    ));
    lines.push(format!(
        "震源：{location} · 深度{:.0}km",
        event.depth_km.unwrap_or_default()
    ));
    lines.push(format!(
        "震级：M{magnitude:.1} · 预计烈度{:.1}",
        timing.estimated_intensity
    ));
    if !event.description.trim().is_empty() && !event.training {
        lines.push(format!("上游信息：{}", event.description.trim()));
    }
    lines.push(String::new());
    lines.push(action_guidance(timing.estimated_intensity).to_string());
    lines.push(String::new());
    lines.push(format!(
        "来源：{} · 第{}报 ({})",
        event.source, event.report_num, event.channel
    ));
    lines.push(format!("通知级别：{} ({level})", level_label(level)));
    lines.push(format!("副标题：{subtitle}"));
    (title, truncate_chars(&lines.join("\n"), MAX_BODY_CHARS))
}

fn arrival_status(seconds: i64) -> String {
    match seconds.cmp(&0) {
        std::cmp::Ordering::Greater => format!("S波{seconds}秒后到达"),
        std::cmp::Ordering::Equal => "S波正在到达".to_string(),
        std::cmp::Ordering::Less => format!("S波已到达{}秒", seconds.saturating_abs()),
    }
}

fn wave_status(seconds: i64) -> String {
    if seconds >= 0 {
        format!("{seconds}秒后到达")
    } else {
        format!("已到达{}秒", seconds.saturating_abs())
    }
}

fn estimated_arrival_time(seconds: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let arrival = now.saturating_add(seconds.max(0) as u64);
    let china_seconds = arrival.saturating_add(8 * 60 * 60) % (24 * 60 * 60);
    let hour = china_seconds / 3600;
    let minute = china_seconds % 3600 / 60;
    let second = china_seconds % 60;
    format!("{hour:02}:{minute:02}:{second:02}")
}

fn intensity_feeling(intensity: f64) -> &'static str {
    if intensity < 1.0 {
        "基本无感"
    } else if intensity < 2.0 {
        "轻微震感"
    } else if intensity < 3.0 {
        "有感震动"
    } else if intensity < 4.0 {
        "明显震感"
    } else if intensity < 5.0 {
        "强烈震感"
    } else {
        "可能造成破坏"
    }
}

fn action_guidance(intensity: f64) -> &'static str {
    if intensity >= 5.0 {
        "可能出现破坏性震动。请立即就近避险，远离玻璃和高处坠物。"
    } else if intensity >= 3.0 {
        "请准备好应对明显震感。保持冷静，寻找安全场所。"
    } else {
        "可能感到震动。请保持冷静，留意后续预警。"
    }
}

fn level_label(level: &str) -> &'static str {
    match level {
        "critical" => "高烈度",
        "active" => "重要通知",
        _ => "普通通知",
    }
}

fn bark_level(level: u8) -> &'static str {
    match level {
        4 => "critical",
        3 => "active",
        _ => "passive",
    }
}

fn subscription_confirmation_summary(subscription: &Subscription) -> (String, String) {
    let target_names = subscription
        .targets
        .iter()
        .map(|target| {
            let name = target.label.trim();
            if name.is_empty() {
                "未命名地点"
            } else {
                name
            }
        })
        .collect::<Vec<_>>();
    let category_names = subscription
        .alerts
        .iter()
        .map(|alert| alert.category().label())
        .collect::<Vec<_>>();
    let subtitle = format!(
        "Bark 通知通道正常 · {} 个地点 · {} 类预警",
        target_names.len(),
        category_names.len()
    );
    let body = format!(
        "监测地点：{}\n预警类型：{}\n请返回网页查看订阅保存结果。",
        target_names.join("、"),
        category_names.join("、")
    );
    (subtitle, body)
}

fn normalize_bark_level(level: &str) -> &'static str {
    match level.trim().to_ascii_lowercase().as_str() {
        "passive" => "passive",
        "active" => "active",
        "timesensitive" => "timeSensitive",
        "critical" => "critical",
        _ => "critical",
    }
}

fn bark_payload(
    message: &BarkMessage<'_>,
    push_config: &BarkPushConfig,
    level: &str,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "device_key": message.device_key,
        "title": message.title,
        "subtitle": message.subtitle,
        "body": message.body,
        "group": push_config.group,
        "level": level,
    });
    if level != "passive" && message.use_alert_sound {
        payload["volume"] = serde_json::json!(push_config.volume);
        if push_config.call {
            payload["call"] = serde_json::json!("1");
        }
        if let Some(sound) = &push_config.sound {
            payload["sound"] = serde_json::json!(sound);
        }
    }
    if let Some(icon_url) = &push_config.icon_url {
        payload["icon"] = serde_json::json!(icon_url);
    }
    payload
}

impl BarkPushConfig {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.volume <= 10, "BARK_VOLUME must be in 0..=10");
        if let Some(icon_url) = &self.icon_url {
            let parsed = Url::parse(icon_url).context("invalid BARK_ICON_URL")?;
            anyhow::ensure!(
                matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
                "BARK_ICON_URL must be an absolute HTTP(S) URL"
            );
        }
        Ok(())
    }
}

fn bark_response_succeeded(body: &str) -> bool {
    if body.trim().is_empty() {
        return true;
    }

    #[derive(Deserialize)]
    struct BarkEnvelope {
        code: Option<i64>,
        success: Option<bool>,
    }

    match serde_json::from_str::<BarkEnvelope>(body) {
        Ok(response) => response.code == Some(200) || response.success == Some(true),
        Err(_) => false,
    }
}

async fn limited_response_text(mut response: reqwest::Response) -> Result<String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        anyhow::bail!("Bark response exceeded size limit");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("failed to read Bark response")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            anyhow::bail!("Bark response exceeded size limit");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn backoff_delay(retry: u32) -> Duration {
    let base = 100u64.saturating_mul(1u64 << retry.saturating_sub(1));
    Duration::from_millis(base + jitter_millis())
}

fn jitter_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::from(duration.subsec_nanos()) % 50)
        .unwrap_or(0)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value
        .chars()
        .take(max_chars.saturating_sub(1))
        .chain(['…'])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        BarkMessage, BarkPushConfig, bark_payload, normalize_bark_level,
        subscription_confirmation_summary, truncate_chars,
    };
    use crate::models::{
        AlertRule, DisasterCategory, GeoPoint, MonitoringTarget, NotificationDestination,
        Subscription,
    };

    #[test]
    fn truncation_preserves_utf8_boundaries() {
        let truncated = truncate_chars("灾害预警abcdef", 6);
        assert_eq!(truncated, "灾害预警a…");
        assert_eq!(truncated.chars().count(), 6);
    }

    #[test]
    fn subscription_confirmation_is_clear_without_coordinates() {
        let subscription = Subscription::new(
            NotificationDestination::Bark {
                base_url: "https://api.day.app".to_string(),
                device_key: "abc123".to_string(),
            },
            vec![MonitoringTarget {
                label: "东京".to_string(),
                point: GeoPoint {
                    latitude: 35.6,
                    longitude: 139.6,
                },
                region: crate::models::AdministrativeRegion::default(),
            }],
            vec![AlertRule::default_for(DisasterCategory::EarthquakeWarning)],
        );

        let (subtitle, body) = subscription_confirmation_summary(&subscription);

        assert_eq!(subtitle, "Bark 通知通道正常 · 1 个地点 · 1 类预警");
        assert!(body.contains("监测地点：东京"));
        assert!(body.contains("预警类型：地震预警"));
        assert!(!body.contains("35.6"));
        assert!(!body.contains("139.6"));
    }

    #[test]
    fn subscription_confirmation_requests_a_banner_without_repeated_ringing() {
        let message = BarkMessage {
            bark_url: "https://api.day.app",
            device_key: "abc123",
            level: "timeSensitive",
            title: "灾害预警接收测试",
            subtitle: "接收测试成功",
            body: "订阅配置正在保存",
            use_alert_sound: false,
        };
        let config = BarkPushConfig {
            sound: Some("alarm".to_string()),
            volume: 10,
            group: "灾害预警".to_string(),
            call: true,
            icon_url: None,
        };
        let level = normalize_bark_level(message.level);

        let payload = bark_payload(&message, &config, level);

        assert_eq!(payload["level"], "timeSensitive");
        assert!(payload.get("sound").is_none());
        assert!(payload.get("volume").is_none());
        assert!(payload.get("call").is_none());
    }

    #[test]
    fn disaster_alerts_keep_the_configured_alert_sound() {
        let message = BarkMessage {
            bark_url: "https://api.day.app",
            device_key: "abc123",
            level: "critical",
            title: "地震预警",
            subtitle: "接收测试",
            body: "测试内容",
            use_alert_sound: true,
        };
        let config = BarkPushConfig {
            sound: Some("alarm".to_string()),
            volume: 10,
            group: "灾害预警".to_string(),
            call: true,
            icon_url: Some("https://alerts.example.com/img/icon.png".to_string()),
        };

        let payload = bark_payload(&message, &config, normalize_bark_level(message.level));

        assert_eq!(payload["level"], "critical");
        assert_eq!(payload["sound"], "alarm");
        assert_eq!(payload["volume"], 10);
        assert_eq!(payload["call"], "1");
        assert_eq!(payload["icon"], "https://alerts.example.com/img/icon.png");
    }
}
