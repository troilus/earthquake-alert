use crate::delivery::{
    NotificationRuleSnapshot, NotificationSnapshot, NotificationSourcesSnapshot,
};
use crate::models::{DisasterEvent, IncidentRecord, IncidentReportSummary};
use axum::{
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use sha2::{Digest as _, Sha256 as Sha256Hasher};
use std::sync::OnceLock;

static DETAIL_CSP: OnceLock<HeaderValue> = OnceLock::new();

pub(crate) fn render_incident_page(
    snapshot: &NotificationSnapshot,
    incident: Option<&IncidentRecord>,
) -> String {
    let mut html = String::with_capacity(24 * 1024);
    html.push_str("<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"color-scheme\" content=\"light\"><title>");
    escape_into(&snapshot.event.title, &mut html);
    html.push_str(" - 灾害详情</title><style>");
    html.push_str(DETAIL_STYLE);
    html.push_str("</style></head><body><header class=\"page-header\"><div class=\"container\"><div class=\"trust-row\"><span class=\"verified\">通知快照签名有效</span><span class=\"category\">");
    escape_into(snapshot.event.category.label(), &mut html);
    html.push_str("</span>");
    if snapshot.event.training {
        html.push_str("<span class=\"training\">演练 / 测试</span>");
    }
    html.push_str("</div><div class=\"title-row\"><div><h1>");
    escape_into(&snapshot.event.title, &mut html);
    html.push_str("</h1><p class=\"headline-meta\">");
    escape_into(&snapshot.event.source, &mut html);
    html.push_str(" · 第 ");
    html.push_str(&snapshot.event.report_num.to_string());
    html.push_str(" 报 · ");
    escape_into(&snapshot.event.occurred_at, &mut html);
    html.push_str("</p></div>");
    html.push_str(incident.map_or_else(
        || status_badge(snapshot.event.cancel, snapshot.event.final_report),
        aggregate_status_badge,
    ));
    html.push_str("</div><p class=\"provenance\">通知依据固定为发送时快照；当前事件状态和报告变化来自服务端后续更新。灾害信息请以官方机构发布为准。</p></div></header><main class=\"container\">");

    html.push_str("<section class=\"overview\" aria-labelledby=\"overview-heading\"><div class=\"section-heading\"><div><span class=\"section-kicker\">事件摘要</span><h2 id=\"overview-heading\">通知时事件概览</h2></div>");
    html.push_str("<span class=\"issued\">签发于 ");
    escape_into(&format_epoch_ms(snapshot.issued_at_ms), &mut html);
    html.push_str("</span></div><dl class=\"fact-grid\">");
    fact("灾害类别", snapshot.event.category.label(), &mut html);
    fact(
        "来源与报告",
        &format!(
            "{} · 第 {} 报",
            snapshot.event.source, snapshot.event.report_num
        ),
        &mut html,
    );
    fact("事件等级", &snapshot.event.level.to_string(), &mut html);
    if let Some(magnitude) = snapshot.event.magnitude {
        fact("震级", &format!("M{magnitude:.1}"), &mut html);
    }
    if let Some(depth) = snapshot.event.depth_km {
        fact("深度", &format!("{depth:.1} km"), &mut html);
    }
    if let Some(radius) = snapshot.event.radius_km {
        fact("七级风圈", &format!("{radius:.0} km"), &mut html);
    }
    if let (Some(latitude), Some(longitude)) = (snapshot.event.latitude, snapshot.event.longitude) {
        fact(
            "事件位置",
            &format!("{latitude:.4}, {longitude:.4}"),
            &mut html,
        );
    }
    if let Some(incident) = incident {
        fact(
            "当前更新",
            &format_epoch_ms(incident.updated_at_ms),
            &mut html,
        );
    }
    html.push_str("</dl>");
    render_regions(&snapshot.event.affected_regions, &mut html);
    if !snapshot.event.description.trim().is_empty() {
        html.push_str("<div class=\"event-description\"><strong>通知时说明</strong><p>");
        escape_into(&snapshot.event.description, &mut html);
        html.push_str("</p></div>");
    }
    html.push_str("</section>");

    html.push_str("<section aria-labelledby=\"impact-heading\"><div class=\"section-heading\"><div><span class=\"section-kicker\">接收依据</span><h2 id=\"impact-heading\">本次通知为何送达</h2></div><span class=\"notification-level ");
    escape_into(&snapshot.interruption_level, &mut html);
    html.push_str("\">");
    escape_into(
        interruption_level_label(&snapshot.interruption_level),
        &mut html,
    );
    html.push_str("通知</span></div><div class=\"detail-columns\"><div><h3>监测地点</h3><dl class=\"data-list\">");
    row("名称", &snapshot.target.label, &mut html);
    row(
        "坐标",
        &format!(
            "{:.6}, {:.6}",
            snapshot.target.latitude, snapshot.target.longitude
        ),
        &mut html,
    );
    let region = [
        snapshot.target.province.as_str(),
        snapshot.target.city.as_str(),
        snapshot.target.district.as_str(),
    ]
    .into_iter()
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join(" / ");
    if !region.is_empty() {
        row("行政区", &region, &mut html);
    }
    html.push_str("</dl></div><div><h3>影响估算</h3><dl class=\"data-list\">");

    if let Some(timing) = snapshot.timing {
        row(
            "预计烈度",
            &format!("{:.2}", timing.estimated_intensity),
            &mut html,
        );
        row(
            "震中距离",
            &format!("{:.2} km", timing.epicentral_distance_km),
            &mut html,
        );
        row(
            "震源距离",
            &format!("{:.2} km", timing.hypocentral_distance_km),
            &mut html,
        );
        row(
            "P 波预计到达",
            &format_epoch_ms(timing.p_arrival_at_ms),
            &mut html,
        );
        row(
            "S 波预计到达",
            &format_epoch_ms(timing.s_arrival_at_ms),
            &mut html,
        );
        html.push_str("</dl>");
    } else {
        html.push_str(
            "</dl><p class=\"empty-note\">本次通知没有附带距离、烈度或到达时间估算。</p>",
        );
    }
    html.push_str(
        "</div></div><div class=\"rule-block\"><h3>命中规则</h3><dl class=\"data-list rule-list\">",
    );

    render_matched_rule(&snapshot.matched_rule, &mut html);
    html.push_str("</dl></div></section>");

    html.push_str("<section aria-labelledby=\"current-heading\"><div class=\"section-heading\"><div><span class=\"section-kicker\">实时汇总</span><h2 id=\"current-heading\">当前事件状态</h2></div>");
    if let Some(incident) = incident {
        html.push_str("<span class=\"source-count\">");
        html.push_str(&incident.latest_by_source.len().to_string());
        html.push_str(" 条最新来源报告</span></div><p class=\"section-context\">首次接收于 ");
        escape_into(&format_epoch_ms(incident.first_seen_at_ms), &mut html);
        html.push_str("，当前事件流状态共 ");
        html.push_str(&incident.stream_watermarks.len().to_string());
        html.push_str(" 条。</p><div class=\"sources\">");
        for event in &incident.latest_by_source {
            render_event(event, &mut html);
        }
        html.push_str("</div>");
    } else {
        html.push_str("</div><div class=\"archive-note\"><strong>当前状态已超出保留期</strong><p>服务端已不再保留该事件的后续状态，以下仍为签名验证通过的通知时快照。</p></div>");
        render_snapshot_event(snapshot, &mut html);
    }
    html.push_str("</section>");

    if let Some(incident) = incident {
        html.push_str("<section aria-labelledby=\"timeline-heading\"><div class=\"section-heading\"><div><span class=\"section-kicker\">变更记录</span><h2 id=\"timeline-heading\">报告变化</h2></div><span class=\"source-count\">最近 ");
        html.push_str(&incident.timeline.len().to_string());
        html.push_str(" 条</span></div><ol class=\"timeline\">");
        for report in incident.timeline.iter().rev() {
            render_report_summary(report, &mut html);
        }
        html.push_str("</ol></section>");
    }
    html.push_str("</main><footer><div class=\"container\"><strong>重要提示</strong><p>数据可能延迟、缺失或误报，不构成官方预警或安全决策依据。</p></div></footer></body></html>");
    html
}

fn render_event(event: &DisasterEvent, html: &mut String) {
    html.push_str("<article class=\"source-report\"><div class=\"article-head\"><div><span class=\"source-label\">");
    escape_into(&event.source, html);
    html.push_str("</span><h3>");
    escape_into(&event.title, html);
    html.push_str("</h3></div><div class=\"report-state\"><span>第 ");
    html.push_str(&event.report_num.to_string());
    html.push_str(" 报</span>");
    html.push_str(status_badge(event.cancel, event.final_report));
    html.push_str("</div></div><dl class=\"data-list event-data\">");
    row("灾害类别", event.category.label(), html);
    row("事件等级", &event.level.to_string(), html);
    if let Some(magnitude) = event.magnitude {
        row("震级", &format!("M{magnitude:.1}"), html);
    }
    if let Some(depth) = event.depth_km {
        row("深度", &format!("{depth:.1} km"), html);
    }
    if let (Some(latitude), Some(longitude)) = (event.latitude, event.longitude) {
        row("位置", &format!("{latitude:.4}, {longitude:.4}"), html);
    }
    if let Some(radius) = event.radius_km {
        row("七级风圈", &format!("{radius:.0} km"), html);
    }
    row("发生时间", &event.occurred_at, html);
    html.push_str("</dl>");
    render_regions(&event.affected_regions, html);
    if !event.description.is_empty() {
        html.push_str("<p class=\"report-description\">");
        escape_into(&event.description, html);
        html.push_str("</p>");
    }
    html.push_str("</article>");
}

fn render_snapshot_event(snapshot: &NotificationSnapshot, html: &mut String) {
    html.push_str("<article class=\"source-report snapshot-report\"><div class=\"article-head\"><div><span class=\"source-label\">通知时来源</span><h3>");
    escape_into(&snapshot.event.title, html);
    html.push_str("</h3></div>");
    html.push_str(status_badge(
        snapshot.event.cancel,
        snapshot.event.final_report,
    ));
    html.push_str("</div><dl class=\"data-list event-data\">");
    row("来源", &snapshot.event.source, html);
    row(
        "报告",
        &format!("第 {} 报", snapshot.event.report_num),
        html,
    );
    if let Some(magnitude) = snapshot.event.magnitude {
        row("震级", &format!("M{magnitude:.1}"), html);
    }
    if let Some(depth) = snapshot.event.depth_km {
        row("深度", &format!("{depth:.1} km"), html);
    }
    if let (Some(latitude), Some(longitude)) = (snapshot.event.latitude, snapshot.event.longitude) {
        row("位置", &format!("{latitude:.4}, {longitude:.4}"), html);
    }
    if let Some(radius) = snapshot.event.radius_km {
        row("七级风圈", &format!("{radius:.0} km"), html);
    }
    row("事件等级", &snapshot.event.level.to_string(), html);
    row("发生时间", &snapshot.event.occurred_at, html);
    html.push_str("</dl>");
    render_regions(&snapshot.event.affected_regions, html);
    if !snapshot.event.description.is_empty() {
        html.push_str("<p class=\"report-description\">");
        escape_into(&snapshot.event.description, html);
        html.push_str("</p>");
    }
    html.push_str("</article>");
}

fn render_matched_rule(rule: &NotificationRuleSnapshot, html: &mut String) {
    match rule {
        NotificationRuleSnapshot::EarthquakeWarning {
            sources,
            intensity_bands,
        } => {
            row("灾害类别", "地震预警", html);
            row("来源范围", &format_sources(sources), html);
            let bands = intensity_bands
                .iter()
                .map(|band| {
                    format!(
                        "{}-{}: {}",
                        band.min,
                        band.max,
                        interruption_level_label(band.interruption_level.as_str())
                    )
                })
                .collect::<Vec<_>>()
                .join("；");
            row("烈度规则", &bands, html);
        }
        NotificationRuleSnapshot::EarthquakeReport {
            sources,
            min_magnitude,
        } => {
            row("灾害类别", "地震速报", html);
            row("来源范围", &format_sources(sources), html);
            row("最低震级", &format!("M{min_magnitude:.1}"), html);
        }
        NotificationRuleSnapshot::WeatherWarning {
            sources,
            min_severity,
            fallback_radius_km,
        } => {
            row("灾害类别", "气象预警", html);
            row("来源范围", &format_sources(sources), html);
            row("最低严重度", &min_severity.to_string(), html);
            row("坐标回退半径", &format!("{fallback_radius_km:.1} km"), html);
        }
        NotificationRuleSnapshot::Tsunami {
            sources,
            min_severity,
        } => {
            row("灾害类别", "海啸预警", html);
            row("来源范围", &format_sources(sources), html);
            row("最低严重度", &min_severity.to_string(), html);
        }
        NotificationRuleSnapshot::Typhoon {
            sources,
            max_center_distance_km,
        } => {
            row("灾害类别", "台风信息", html);
            row("来源范围", &format_sources(sources), html);
            row(
                "中心最大距离",
                &format!("{max_center_distance_km:.1} km"),
                html,
            );
        }
    }
}

fn format_sources(sources: &NotificationSourcesSnapshot) -> String {
    match sources {
        NotificationSourcesSnapshot::All => "全部来源".to_string(),
        NotificationSourcesSnapshot::Include(ids) => ids.join("、"),
    }
}

fn interruption_level_label(value: &str) -> &'static str {
    match value {
        "passive" => "静默",
        "active" => "主动",
        "critical" => "紧急",
        _ => "未知",
    }
}

fn render_report_summary(report: &IncidentReportSummary, html: &mut String) {
    html.push_str("<li><time>");
    escape_into(&format_epoch_ms(report.observed_at_ms), html);
    html.push_str("</time><div class=\"timeline-content\"><div class=\"timeline-title\"><strong>");
    escape_into(&report.source, html);
    html.push_str(" · 第 ");
    html.push_str(&report.report_num.to_string());
    html.push_str(" 报</strong>");
    html.push_str(status_badge(report.cancel, report.final_report));
    html.push_str("</div><p>");
    if let Some(magnitude) = report.magnitude {
        html.push_str(&format!("M{magnitude:.1} "));
    }
    if let Some(depth) = report.depth_km {
        html.push_str(&format!("深度{depth:.1}km "));
    }
    if let (Some(latitude), Some(longitude)) = (report.latitude, report.longitude) {
        html.push_str(&format!("{latitude:.3}, {longitude:.3} "));
    }
    html.push_str("等级 ");
    html.push_str(&report.level.to_string());
    html.push_str("</p></div></li>");
}

fn render_regions(regions: &[String], html: &mut String) {
    if regions.is_empty() {
        return;
    }
    html.push_str("<div class=\"regions\"><strong>影响区域</strong><ul>");
    for region in regions {
        html.push_str("<li>");
        escape_into(region, html);
        html.push_str("</li>");
    }
    html.push_str("</ul></div>");
}

fn fact(label: &str, value: &str, html: &mut String) {
    html.push_str("<div><dt>");
    escape_into(label, html);
    html.push_str("</dt><dd>");
    escape_into(value, html);
    html.push_str("</dd></div>");
}

fn status_badge(cancel: bool, final_report: bool) -> &'static str {
    if cancel {
        "<span class=\"status cancel\">已解除</span>"
    } else if final_report {
        "<span class=\"status final\">终报</span>"
    } else {
        "<span class=\"status active\">进行中</span>"
    }
}

fn aggregate_status_badge(incident: &IncidentRecord) -> &'static str {
    let stream_count = incident.stream_watermarks.len();
    let cancelled = incident
        .stream_watermarks
        .iter()
        .filter(|stream| stream.cancel)
        .count();
    let final_reports = incident
        .stream_watermarks
        .iter()
        .filter(|stream| !stream.cancel && stream.final_report)
        .count();
    let active = stream_count
        .saturating_sub(cancelled)
        .saturating_sub(final_reports);
    if stream_count > 0 && cancelled == stream_count {
        status_badge(true, false)
    } else if stream_count > 0 && final_reports == stream_count {
        status_badge(false, true)
    } else if stream_count > 0 && active == stream_count {
        status_badge(false, false)
    } else {
        "<span class=\"status mixed\">来源状态不一致</span>"
    }
}

fn row(label: &str, value: &str, html: &mut String) {
    html.push_str("<dt>");
    escape_into(label, html);
    html.push_str("</dt><dd>");
    escape_into(value, html);
    html.push_str("</dd>");
}

fn escape_into(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
}

fn format_epoch_ms(value: i64) -> String {
    let seconds = value.div_euclid(1_000);
    let millis = value.rem_euclid(1_000);
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3_600;
    let minute = day_seconds % 3_600 / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{millis:03} UTC")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

pub(crate) fn detail_response(html: String) -> Response {
    let mut response = Html(html).into_response();
    apply_detail_headers(response.headers_mut());
    response
}

pub(crate) fn detail_not_found() -> Response {
    let mut response = detail_message_response(
        StatusCode::NOT_FOUND,
        "链接无效",
        "链接无效或内容无法验证。请从原始通知重新打开详情。",
    );
    apply_detail_headers(response.headers_mut());
    response
}

pub(crate) fn detail_error() -> Response {
    let mut response = detail_message_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "暂时无法加载",
        "详情暂时无法加载，请稍后重试。通知时快照不会因此改变。",
    );
    apply_detail_headers(response.headers_mut());
    response
}

pub(crate) fn detail_unavailable() -> Response {
    let mut response = detail_message_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "请稍后重试",
        "详情请求较多，请稍后重试。服务正在保护其他事件处理任务。",
    );
    apply_detail_headers(response.headers_mut());
    response
}

fn detail_message_response(status: StatusCode, title: &str, message: &str) -> Response {
    let mut html = String::with_capacity(2_048);
    html.push_str("<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>");
    escape_into(title, &mut html);
    html.push_str(" - 灾害详情</title><style>");
    html.push_str(DETAIL_STYLE);
    html.push_str("</style></head><body><header class=\"page-header\"><div class=\"container\"><div class=\"trust-row\"><span class=\"category\">灾害详情</span></div><h1>");
    escape_into(title, &mut html);
    html.push_str("</h1><p class=\"provenance\">当前无法提供所请求的灾害详情。</p></div></header><main class=\"container\"><section class=\"message-panel\"><span class=\"message-mark\" aria-hidden=\"true\">!</span><div><h2>无法显示详情</h2><p>");
    escape_into(message, &mut html);
    html.push_str("</p></div></section></main><footer><div class=\"container\"><strong>重要提示</strong><p>详情页不会缓存通知接收者信息。</p></div></footer></body></html>");
    (status, Html(html)).into_response()
}

fn apply_detail_headers(headers: &mut HeaderMap) {
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(header::EXPIRES, HeaderValue::from_static("0"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow, noarchive"),
    );
    let csp = DETAIL_CSP.get_or_init(|| {
        let digest = Sha256Hasher::digest(DETAIL_STYLE.as_bytes());
        let policy = format!(
            "default-src 'none'; style-src 'sha256-{}'; img-src 'none'; font-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            BASE64_STANDARD.encode(digest)
        );
        HeaderValue::from_str(&policy).unwrap_or_else(|error| {
            tracing::error!(
                event = "incident.detail_csp_invalid",
                error = ?error,
                "incident.detail_csp_invalid"
            );
            HeaderValue::from_static(
                "default-src 'none'; style-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            )
        })
    });
    headers.insert("content-security-policy", csp.clone());
}

const DETAIL_STYLE: &str = "*{box-sizing:border-box}html{background:#eef2f1}body{min-height:100vh;display:flex;flex-direction:column;margin:0;background:#f5f7f6;color:#182326;font:15px/1.55 system-ui,-apple-system,BlinkMacSystemFont,\"Segoe UI\",sans-serif}a{color:inherit}.container{width:min(100% - 32px,960px);margin:0 auto}.page-header{background:#173c3b;color:#f7fbf8;border-bottom:4px solid #e5b84b}.page-header .container{padding:28px 0 24px}.trust-row,.title-row,.section-heading,.article-head,.timeline-title{display:flex;align-items:flex-start;justify-content:space-between;gap:16px}.trust-row{align-items:center;justify-content:flex-start;flex-wrap:wrap;font-size:13px}.verified{margin-right:auto;color:#b8e4bf;font-weight:700}.category,.training,.notification-level,.status{display:inline-flex;align-items:center;min-height:26px;padding:3px 9px;border-radius:4px;font-size:12px;font-weight:700;white-space:nowrap}.category{background:#275857;color:#dff5e4}.training{background:#7b5424;color:#fff1bd}.title-row{margin-top:18px;align-items:center}.title-row>div{min-width:0}h1{max-width:720px;margin:0;font-size:30px;line-height:1.2;overflow-wrap:anywhere}h2{margin:0;font-size:20px;line-height:1.25}h3{margin:0 0 12px;font-size:16px;line-height:1.35}.headline-meta,.provenance{margin:8px 0 0;color:#c2d3ce}.provenance{margin-top:20px;padding-top:16px;border-top:1px solid #39605e;font-size:13px}.status{border:1px solid transparent}.active{background:#fff0c2;color:#704d00}.final{background:#dcecff;color:#154c79}.cancel{background:#dfe8e2;color:#28583b}.mixed{background:#ffe1d7;color:#7a2c19}main{flex:1;display:grid;align-content:start;gap:16px;padding:24px 0 36px}section{background:#fff;border:1px solid #d9e2df;border-radius:6px;padding:24px;box-shadow:0 1px 2px rgba(18,49,45,.04)}.section-heading{align-items:center;margin-bottom:20px}.section-kicker{display:block;margin-bottom:4px;color:#35706a;font-size:12px;font-weight:700;letter-spacing:0}.issued,.source-count{color:#6b7978;font-size:13px;text-align:right}.fact-grid{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:16px 20px;margin:0}.fact-grid>div{min-width:0;padding-top:12px;border-top:1px solid #e3eae8}.fact-grid dt,.data-list dt{color:#6b7978;font-size:13px}.fact-grid dd{margin:3px 0 0;font-weight:650;overflow-wrap:anywhere}.regions{display:flex;gap:12px;align-items:flex-start;margin-top:20px;padding-top:16px;border-top:1px solid #e3eae8}.regions strong{flex:0 0 auto;color:#536765;font-size:13px}.regions ul{display:flex;flex-wrap:wrap;gap:6px 8px;padding:0;margin:0;list-style:none}.regions li{padding:3px 8px;background:#edf4f1;border:1px solid #d4e4de;border-radius:3px;color:#315952;font-size:13px}.event-description,.report-description{margin:18px 0 0;padding:14px 16px;background:#f6f9f8;border-left:3px solid #9dc6b5}.event-description strong{font-size:13px;color:#48615d}.event-description p,.report-description{margin-bottom:0;white-space:pre-wrap;overflow-wrap:anywhere}.detail-columns{display:grid;grid-template-columns:1fr 1fr;gap:24px}.detail-columns>div+div{padding-left:24px;border-left:1px solid #e3eae8}.data-list{display:grid;grid-template-columns:minmax(100px,130px) 1fr;gap:9px 14px;margin:0}.data-list dd{margin:0;overflow-wrap:anywhere}.notification-level{background:#fff0c2;color:#704d00}.rule-block{margin-top:24px;padding-top:20px;border-top:1px solid #e3eae8}.rule-list{grid-template-columns:minmax(110px,160px) 1fr}.empty-note,.section-context,.archive-note p{margin:0;color:#6b7978;font-size:13px}.sources{display:grid;gap:0}.source-report{padding:18px 0;border-top:1px solid #e1e9e6}.source-report:first-child{padding-top:0;border-top:0}.article-head{align-items:flex-start}.article-head>div:first-child{min-width:0}.source-label{display:block;color:#35706a;font-size:13px;font-weight:700}.source-report h3{margin:4px 0 0;overflow-wrap:anywhere}.report-state{display:flex;align-items:flex-end;flex-direction:column;gap:8px;flex:0 0 auto;color:#60716e;font-size:13px}.event-data{margin-top:14px}.snapshot-report{padding-top:0}.archive-note{display:flex;gap:12px;margin-bottom:20px;padding:14px 16px;background:#fff8e6;border:1px solid #eddbad;border-radius:4px;color:#634d1e}.archive-note strong{display:block}.archive-note p{margin:2px 0 0;color:#735e31}.archive-note:before{content:'i';display:grid;place-items:center;flex:0 0 22px;height:22px;border:1px solid #c8a75f;border-radius:50%;font-weight:700}.timeline{list-style:none;padding:0;margin:0}.timeline li{display:grid;grid-template-columns:180px 1fr;gap:18px;padding:16px 0;border-top:1px solid #e1e9e6}.timeline li:first-child{border-top:0;padding-top:0}.timeline time{color:#6b7978;font-size:13px}.timeline-content{min-width:0}.timeline-title{align-items:center}.timeline-title strong{overflow-wrap:anywhere}.timeline-title .status{flex:0 0 auto}.timeline p{margin:6px 0 0;color:#596966;font-size:13px}.message-panel{display:flex;gap:16px;align-items:flex-start}.message-panel h2{margin:0 0 8px}.message-panel p{margin:0;color:#61706e}.message-mark{display:grid;place-items:center;flex:0 0 32px;height:32px;background:#ffe6dc;color:#9a3c24;border-radius:50%;font-weight:800}footer{border-top:1px solid #d9e2df;background:#edf2f0;color:#667572;font-size:13px}footer .container{padding:20px 0 28px}footer strong{color:#40524e}footer p{margin:4px 0 0}@media(max-width:760px){.page-header .container{padding:22px 0}.title-row{display:block}.title-row>.status{display:inline-flex;margin-top:14px}.fact-grid{grid-template-columns:repeat(2,minmax(0,1fr))}.detail-columns{grid-template-columns:1fr;gap:22px}.detail-columns>div+div{padding-left:0;border-left:0;padding-top:22px;border-top:1px solid #e3eae8}.timeline li{grid-template-columns:1fr;gap:5px}.timeline-title{align-items:flex-start}.container{width:min(100% - 24px,960px)}section{padding:18px}main{padding-top:12px}}@media(max-width:430px){h1{font-size:24px}.section-heading{display:block}.issued,.source-count{display:block;margin-top:8px;text-align:left}.fact-grid{gap:12px}.data-list,.rule-list{grid-template-columns:1fr;gap:2px}.data-list dd{margin-bottom:8px}.regions{display:block}.regions ul{margin-top:8px}.article-head{display:block}.report-state{align-items:flex-start;flex-direction:row;margin-top:10px}.message-panel{gap:12px}}";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::{
        NotificationEventSnapshot, NotificationIntensityBandSnapshot, NotificationTargetSnapshot,
        NotificationTimingSnapshot,
    };
    use crate::models::InterruptionLevel;
    use crate::models::{DisasterCategory, IncidentId, ProviderChannel};

    fn event(source: &str) -> DisasterEvent {
        DisasterEvent {
            category: DisasterCategory::EarthquakeWarning,
            channel: ProviderChannel::FanStudio,
            source: source.to_string(),
            event_id: format!("{source}-event"),
            revision: "1".to_string(),
            report_num: 1,
            title: "<script>alert(1)</script>".to_string(),
            description: "A & B".to_string(),
            latitude: Some(35.0),
            longitude: Some(139.0),
            magnitude: Some(5.0),
            depth_km: Some(10.0),
            affected_regions: Vec::new(),
            radius_km: None,
            level: 3,
            occurred_at: "2026-07-12 12:00:00".to_string(),
            final_report: false,
            cancel: false,
            training: false,
        }
    }

    fn snapshot() -> NotificationSnapshot {
        NotificationSnapshot {
            schema_version: 1,
            incident_id: IncidentId::derive("source:event"),
            issued_at_ms: 0,
            event: NotificationEventSnapshot {
                category: DisasterCategory::EarthquakeWarning,
                source: "source".to_string(),
                source_event_id: "event".to_string(),
                revision: "1".to_string(),
                report_num: 1,
                title: "<script>alert(1)</script>".to_string(),
                description: "A & B".to_string(),
                affected_regions: vec!["<东京>".to_string()],
                latitude: Some(35.0),
                longitude: Some(139.0),
                magnitude: Some(5.0),
                depth_km: Some(10.0),
                radius_km: Some(120.0),
                level: 3,
                occurred_at: "2026-07-12 12:00:00".to_string(),
                final_report: false,
                cancel: false,
                training: false,
            },
            target: NotificationTargetSnapshot {
                label: "<住所>".to_string(),
                latitude: 35.6,
                longitude: 139.6,
                province: "东京都".to_string(),
                city: "东京".to_string(),
                district: String::new(),
            },
            timing: Some(NotificationTimingSnapshot {
                epicentral_distance_km: 50.0,
                hypocentral_distance_km: 51.0,
                estimated_intensity: 3.2,
                p_arrival_at_ms: 1_000,
                s_arrival_at_ms: 2_000,
            }),
            interruption_level: "critical".to_string(),
            matched_rule: NotificationRuleSnapshot::EarthquakeWarning {
                sources: NotificationSourcesSnapshot::All,
                intensity_bands: vec![NotificationIntensityBandSnapshot {
                    min: 3,
                    max: 7,
                    interruption_level: InterruptionLevel::Critical,
                }],
            },
        }
    }

    #[test]
    fn page_escapes_signed_and_stored_dynamic_content() {
        let snapshot = snapshot();
        let mut incident = IncidentRecord::new(snapshot.incident_id.clone(), &event("source-a"), 1);
        let mut cancelled = event("source-b");
        cancelled.cancel = true;
        assert!(incident.apply(&cancelled, 2));

        let html = render_incident_page(&snapshot, Some(&incident));

        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&lt;住所&gt;"));
        assert!(html.contains("A &amp; B"));
        assert!(html.contains("来源状态不一致"));
        assert!(html.contains("命中规则"));
        assert!(html.contains("3-7: 紧急"));
        assert!(html.contains("notification-level critical\">紧急通知"));
        assert!(html.contains("通知时事件概览"));
        assert!(html.contains("本次通知为何送达"));
        assert!(html.contains("当前事件状态"));
        assert!(html.contains("报告变化"));
        assert!(html.contains("七级风圈</dt><dd>120 km"));
        assert!(html.contains("<li>&lt;东京&gt;</li>"));
        assert!(html.contains("2 条最新来源报告"));
    }

    #[test]
    fn page_falls_back_to_the_verified_snapshot_after_incident_retention() {
        let html = render_incident_page(&snapshot(), None);

        assert!(html.contains("当前状态已超出保留期"));
        assert!(html.contains("通知时快照"));
        assert!(html.contains("七级风圈</dt><dd>120 km"));
        assert!(html.contains("<span class=\"source-label\">通知时来源</span>"));
        assert!(!html.contains("id=\"timeline-heading\""));
    }

    #[test]
    fn aggregate_status_uses_non_evictable_stream_watermarks() {
        let snapshot = snapshot();
        let mut active = event("source-0");
        active.final_report = false;
        let mut incident = IncidentRecord::new(snapshot.incident_id, &active, 1);
        for index in 1..=8 {
            let mut terminal = event(&format!("source-{index}"));
            terminal.final_report = true;
            assert!(incident.apply(&terminal, index + 1));
        }
        assert_eq!(incident.latest_by_source.len(), 8);
        assert_eq!(incident.stream_watermarks.len(), 9);
        assert!(aggregate_status_badge(&incident).contains("mixed"));
    }

    #[test]
    fn utc_formatter_handles_epoch_and_negative_milliseconds() {
        assert_eq!(format_epoch_ms(0), "1970-01-01 00:00:00.000 UTC");
        assert_eq!(format_epoch_ms(-1), "1969-12-31 23:59:59.999 UTC");
        assert_eq!(
            format_epoch_ms(1_783_814_400_123),
            "2026-07-12 00:00:00.123 UTC"
        );
    }

    #[test]
    fn detail_responses_apply_private_security_headers() {
        let response = detail_response("ok".to_string());
        let headers = response.headers();
        assert_eq!(
            headers
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("private, no-store")
        );
        assert_eq!(
            headers
                .get(header::REFERRER_POLICY)
                .and_then(|value| value.to_str().ok()),
            Some("no-referrer")
        );
        assert_eq!(
            headers
                .get(header::PRAGMA)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache")
        );
        assert!(headers.get("content-security-policy").is_some_and(|value| {
            value.to_str().is_ok_and(|value| {
                value.contains("default-src 'none'") && value.contains("frame-ancestors 'none'")
            })
        }));
    }

    #[test]
    fn content_security_policy_hash_matches_the_inline_stylesheet() {
        let expected = format!(
            "sha256-{}",
            BASE64_STANDARD.encode(Sha256Hasher::digest(DETAIL_STYLE))
        );
        let response = detail_response("ok".to_string());
        let policy = response
            .headers()
            .get("content-security-policy")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");

        assert!(policy.contains(&expected));
        assert!(!policy.contains("'unsafe-inline'"));
    }

    #[tokio::test]
    async fn error_pages_are_styled_clear_and_private() -> anyhow::Result<()> {
        for (response, expected_status, expected_message) in [
            (detail_not_found(), StatusCode::NOT_FOUND, "链接无效"),
            (
                detail_error(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "暂时无法加载",
            ),
            (
                detail_unavailable(),
                StatusCode::SERVICE_UNAVAILABLE,
                "请稍后重试",
            ),
        ] {
            anyhow::ensure!(response.status() == expected_status);
            anyhow::ensure!(
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok())
                    == Some("private, no-store")
            );
            let body = axum::body::to_bytes(response.into_body(), 32 * 1024).await?;
            let body = std::str::from_utf8(&body)?;
            anyhow::ensure!(body.contains(DETAIL_STYLE));
            anyhow::ensure!(body.contains(expected_message));
            anyhow::ensure!(body.contains("无法显示详情"));
        }
        Ok(())
    }
}
