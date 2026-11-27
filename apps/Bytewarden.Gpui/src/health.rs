//! Zero-Retention Vault Security Health & Password Audit Page.
//!
//! Scans active Login items on a background worker, immediately revoking each
//! `Detail` lease and wiping plaintext fields in-place. Password reuse is detected
//! via ephemeral 128-bit CSPRNG-keyed SipHash tags whose keys are discarded before
//! returning, so zero plaintext or offline-guessable hashes ever reside in UI memory.

use std::{
    collections::{HashMap, hash_map::RandomState},
    hash::{BuildHasher, Hasher},
};

use bytewarden_core::desktop::{Request, Session};
use gpui::{Context, Div, FontWeight, Stateful, div, prelude::*, px};
use zeroize::Zeroize;

use crate::{
    Page, VaultPreview,
    theme::{self, hex},
    ui::{
        components::{BtnVariant, badge, btn, card_group, elevated_card, glyph_icon, pill_chip},
        tokens::{font, glyph, space},
    },
    vault_list::Filter,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HealthFilter {
    All,
    Weak,
    Reused,
    MissingTwoFactor,
    InsecureHttp,
    Stale,
}

impl HealthFilter {
    pub const ALL: [(Self, &'static str); 6] = [
        (Self::All, "全部风险"),
        (Self::Weak, "弱密码"),
        (Self::Reused, "重复密码"),
        (Self::MissingTwoFactor, "未开 2FA"),
        (Self::InsecureHttp, "HTTP 明文"),
        (Self::Stale, "超一年未换"),
    ];
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    Weak,
    Reused(usize),
    MissingTwoFactor,
    InsecureHttp,
    Stale,
}

impl IssueKind {
    fn label(self) -> String {
        match self {
            Self::Weak => "弱密码".into(),
            Self::Reused(n) => format!("重复使用 ({n} 处)"),
            Self::MissingTwoFactor => "未启用 2FA / 通行密钥".into(),
            Self::InsecureHttp => "不安全 HTTP 链接".into(),
            Self::Stale => "超过 1 年未更换".into(),
        }
    }

    fn matches(self, filter: HealthFilter) -> bool {
        match filter {
            HealthFilter::All => true,
            HealthFilter::Weak => matches!(self, Self::Weak),
            HealthFilter::Reused => matches!(self, Self::Reused(_)),
            HealthFilter::MissingTwoFactor => matches!(self, Self::MissingTwoFactor),
            HealthFilter::InsecureHttp => matches!(self, Self::InsecureHttp),
            HealthFilter::Stale => matches!(self, Self::Stale),
        }
    }

    fn is_high_severity(self) -> bool {
        matches!(self, Self::Weak | Self::Reused(_))
    }
}

pub struct HealthFinding {
    pub item_id: String,
    pub name: String,
    pub subtitle: String,
    pub issues: Vec<IssueKind>,
}

pub struct HealthReport {
    pub total_logins: usize,
    pub scanned_logins: usize,
    pub skipped_reprompt: usize,
    pub score: u8,
    pub weak_count: usize,
    pub reused_count: usize,
    pub missing_2fa_count: usize,
    pub http_count: usize,
    pub stale_count: usize,
    pub findings: Vec<HealthFinding>,
}

pub struct HealthPage {
    pub scanning: bool,
    pub filter: HealthFilter,
    pub scanned_generation: Option<u64>,
    pub report: Option<HealthReport>,
}

impl Default for HealthPage {
    fn default() -> Self {
        Self {
            scanning: false,
            filter: HealthFilter::All,
            scanned_generation: None,
            report: None,
        }
    }
}

impl HealthPage {
    pub fn clear(&mut self) {
        self.scanning = false;
        self.filter = HealthFilter::All;
        self.scanned_generation = None;
        self.report = None;
    }
}

fn is_insecure_http_uri(raw: &str) -> bool {
    let trimmed = raw.trim();
    if !trimmed
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case("http://"))
    {
        return false;
    }
    let Some(host) = crate::browser_bridge::extract_url_host(trimmed) else {
        return false;
    };
    !matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1") && !host.ends_with(".localhost")
}

fn is_older_than_one_year(iso: &str, now_ts: i64) -> bool {
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .is_some_and(|dt| now_ts.saturating_sub(dt.timestamp()) > 365 * 86_400)
}

fn ephemeral_tag(s1: &RandomState, s2: &RandomState, secret: &str) -> (u64, u64) {
    let mut h1 = s1.build_hasher();
    h1.write(secret.as_bytes());
    let mut h2 = s2.build_hasher();
    h2.write(secret.as_bytes());
    (h1.finish(), h2.finish())
}

impl VaultPreview {
    pub(crate) fn run_health_scan(&mut self, cx: &mut Context<Self>) {
        if self.health.scanning || self.client.state() != Session::Unlocked {
            return;
        }
        let Some(overview) = &self.client.overview else {
            return;
        };
        let generation = self.client.overview_generation;
        let candidates: Vec<(String, String, String, String, bool)> = overview
            .items
            .iter()
            .filter(|i| !i.deleted && !i.restricted && i.item_type == 1)
            .map(|i| {
                (
                    i.id.clone(),
                    i.name.clone(),
                    i.subtitle.clone(),
                    i.revision_date.clone(),
                    i.reprompt,
                )
            })
            .collect();

        self.health.scanning = true;
        cx.notify();
        let bg = cx.background_executor().clone();

        cx.spawn(async move |weak, app| {
            let report = bg
                .spawn(async move { compute_health_report(candidates) })
                .await;
            let _ = weak.update(app, |this, cx| {
                this.health.scanning = false;
                if this.client.state() == Session::Unlocked
                    && this.client.overview_generation == generation
                {
                    this.health.scanned_generation = Some(generation);
                    this.health.report = Some(report);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn jump_to_vault_item(&mut self, item_id: &str, cx: &mut Context<Self>) {
        let Some(overview) = &self.client.overview else {
            return;
        };
        let Some(index) = overview.items.iter().position(|i| i.id == item_id) else {
            return;
        };
        self.filter = Filter::All;
        self.client.selected_folder = None;
        self.query.clear();
        self.client.search.update(cx, |input, cx| input.set("", cx));
        self.invalidate_query();
        self.open_page(Page::Vault, cx);
        self.choose_item(index, cx);
    }

    pub(crate) fn render_health(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let pal = theme::palette();
        let unlocked = self.client.state() == Session::Unlocked && self.client.overview.is_some();

        // Automatically kick off first scan when opening Health page while unlocked.
        if unlocked
            && !self.health.scanning
            && self.health.scanned_generation != Some(self.client.overview_generation)
        {
            self.run_health_scan(cx);
        }

        let content = if !unlocked {
            elevated_card()
                .p(px(space::XL))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .text_color(hex(pal.accent_text))
                        .child(glyph_icon(glyph::SHIELD, 28.)),
                )
                .child(
                    div()
                        .text_size(px(font::BODY_LG))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(hex(pal.text))
                        .child("解锁保险库以运行安全体检"),
                )
                .child(
                    div()
                        .text_size(px(font::CAPTION))
                        .text_color(hex(pal.text_secondary))
                        .child("本机离线扫描弱密码、重复密码、未启用 2FA 与 HTTP 不安全站点，零数据上传且不留存明文。"),
                )
        } else {
            let mut stack = div().flex().flex_col().gap(px(space::L));

            // Header bar
            stack = stack.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .text_size(px(font::SUBTITLE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hex(pal.text))
                                    .child("密码库安全体检"),
                            )
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child("全部检测均在本机内存瞬时完成并即刻擦除明文，不会向任何服务器发送密码。"),
                            ),
                    )
                    .child(
                        btn("health-rescan", BtnVariant::Secondary)
                            .h(px(32.))
                            .px(px(12.))
                            .when(self.health.scanning, |b| b.opacity(0.5))
                            .child(glyph_icon(glyph::SYNC, 13.))
                            .child(if self.health.scanning { "正在扫描…" } else { "重新体检" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run_health_scan(cx);
                            })),
                    ),
            );

            if let Some(rep) = &self.health.report {
                let score_color = if rep.score >= 85 {
                    pal.success
                } else if rep.score >= 60 {
                    pal.warning
                } else {
                    pal.danger
                };
                let grade = if rep.score >= 90 {
                    "极佳"
                } else if rep.score >= 75 {
                    "良好"
                } else if rep.score >= 55 {
                    "一般"
                } else {
                    "需尽快加固"
                };

                // Score hero + metric counters
                stack = stack.child(
                    elevated_card()
                        .p(px(space::L))
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .gap(px(10.))
                                        .child(
                                            div()
                                                .text_size(px(34.))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(hex(score_color))
                                                .child(format!("{}", rep.score)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(font::BODY))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(hex(pal.text))
                                                .child(format!("/ 100 分 · {grade}")),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(font::CAPTION))
                                        .text_color(hex(pal.text_muted))
                                        .child(if rep.skipped_reprompt > 0 {
                                            format!(
                                                "已检测 {} / {} 项（跳过 {} 项主密码保护条目）",
                                                rep.scanned_logins,
                                                rep.total_logins,
                                                rep.skipped_reprompt
                                            )
                                        } else {
                                            format!("已检测全部 {} 个登录条目", rep.scanned_logins)
                                        }),
                                ),
                        )
                        .child(
                            div().flex().flex_wrap().gap(px(6.)).children(
                                HealthFilter::ALL.into_iter().enumerate().map(
                                    |(idx, (f, label))| {
                                        let count = match f {
                                            HealthFilter::All => rep.findings.len(),
                                            HealthFilter::Weak => rep.weak_count,
                                            HealthFilter::Reused => rep.reused_count,
                                            HealthFilter::MissingTwoFactor => rep.missing_2fa_count,
                                            HealthFilter::InsecureHttp => rep.http_count,
                                            HealthFilter::Stale => rep.stale_count,
                                        };
                                        pill_chip(
                                            ("health-filter", idx),
                                            format!("{label} ({count})"),
                                            None,
                                            self.health.filter == f,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.health.filter = f;
                                                cx.notify();
                                            }),
                                        )
                                    },
                                ),
                            ),
                        ),
                );

                let active_filter = self.health.filter;
                let filtered: Vec<&HealthFinding> = rep
                    .findings
                    .iter()
                    .filter(|f| f.issues.iter().any(|k| k.matches(active_filter)))
                    .collect();

                if filtered.is_empty() {
                    stack = stack.child(
                        elevated_card()
                            .p(px(space::XL))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_color(hex(pal.success))
                                    .child(glyph_icon(glyph::CHECK, 24.)),
                            )
                            .child(
                                div()
                                    .text_size(px(font::BODY))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(hex(pal.text))
                                    .child("当前分类下未发现任何安全风险"),
                            ),
                    );
                } else {
                    let mut group = card_group("待优化条目（点击可直达修改）", Some(glyph::SHIELD));
                    for (idx, finding) in filtered.iter().enumerate() {
                        let item_id = finding.item_id.clone();
                        let is_last = idx + 1 == filtered.len();
                        group = group.child(
                            div()
                                .id(("health-item", idx))
                                .cursor_pointer()
                                .px(px(space::L))
                                .py(px(12.))
                                .when(!is_last, |r| {
                                    r.border_b_1().border_color(hex(pal.border_subtle))
                                })
                                .hover(move |s| s.bg(hex(pal.hover)))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.jump_to_vault_item(&item_id, cx);
                                }))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap(px(3.))
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(font::BODY))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(hex(pal.text))
                                                .child(finding.name.clone()),
                                        )
                                        .when(!finding.subtitle.is_empty(), |col| {
                                            col.child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(font::CAPTION))
                                                    .text_color(hex(pal.text_secondary))
                                                    .child(finding.subtitle.clone()),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .items_center()
                                        .justify_end()
                                        .gap(px(6.))
                                        .children(finding.issues.iter().map(|issue| {
                                            badge(
                                                issue.label(),
                                                if issue.is_high_severity() {
                                                    pal.danger
                                                } else {
                                                    pal.warning
                                                },
                                            )
                                        }))
                                        .child(
                                            div()
                                                .text_color(hex(pal.text_muted))
                                                .child(glyph_icon(glyph::CHEVRON_RIGHT, 12.)),
                                        ),
                                ),
                        );
                    }
                    stack = stack.child(group);
                }
            }
            stack
        };

        div()
            .id("health-page-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(space::XL))
            .child(div().w_full().max_w(px(780.)).mx_auto().child(content))
    }
}

fn compute_health_report(candidates: Vec<(String, String, String, String, bool)>) -> HealthReport {
    let total_logins = candidates.len();
    let mut scanned_logins = 0usize;
    let mut skipped_reprompt = 0usize;

    let s1 = RandomState::new();
    let s2 = RandomState::new();
    let now_ts = chrono::Utc::now().timestamp();

    struct RawEntry {
        id: String,
        name: String,
        subtitle: String,
        pw_tag: Option<(u64, u64)>,
        weak: bool,
        missing_2fa: bool,
        http: bool,
        stale: bool,
    }

    let mut raw_entries = Vec::with_capacity(total_logins);
    let mut tag_counts: HashMap<(u64, u64), usize> = HashMap::new();

    for (id, name, subtitle, rev_date, reprompt) in candidates {
        if reprompt {
            skipped_reprompt += 1;
            continue;
        }
        let req = Request::begin();
        let Ok(mut detail) = req.detail(&id, "") else {
            continue;
        };
        scanned_logins += 1;

        let mut pw_tag = None;
        let mut has_pw = false;
        let mut http = false;

        for field in &mut detail.fields {
            if field.label == "密码" && !field.value.is_empty() {
                has_pw = true;
                let tag = ephemeral_tag(&s1, &s2, &field.value);
                *tag_counts.entry(tag).or_insert(0) += 1;
                pw_tag = Some(tag);
            } else if field.label == "网站" && is_insecure_http_uri(&field.value) {
                http = true;
            }
            field.value.zeroize();
        }

        let weak = has_pw && detail.password_strength.is_some_and(|score| score <= 2);
        let missing_2fa = detail.totp.is_none() && detail.passkeys.is_empty();
        let effective_date = detail
            .password_revision_date
            .as_deref()
            .unwrap_or(&rev_date);
        let stale = has_pw && is_older_than_one_year(effective_date, now_ts);

        detail.revoke();

        raw_entries.push(RawEntry {
            id,
            name,
            subtitle,
            pw_tag,
            weak,
            missing_2fa,
            http,
            stale,
        });
    }

    let mut weak_count = 0usize;
    let mut reused_count = 0usize;
    let mut missing_2fa_count = 0usize;
    let mut http_count = 0usize;
    let mut stale_count = 0usize;
    let mut findings = Vec::new();

    for entry in raw_entries {
        let mut issues = Vec::new();
        if entry.weak {
            weak_count += 1;
            issues.push(IssueKind::Weak);
        }
        if let Some(tag) = entry.pw_tag
            && let Some(&count) = tag_counts.get(&tag)
            && count > 1
        {
            reused_count += 1;
            issues.push(IssueKind::Reused(count));
        }
        if entry.missing_2fa {
            missing_2fa_count += 1;
            issues.push(IssueKind::MissingTwoFactor);
        }
        if entry.http {
            http_count += 1;
            issues.push(IssueKind::InsecureHttp);
        }
        if entry.stale {
            stale_count += 1;
            issues.push(IssueKind::Stale);
        }
        if !issues.is_empty() {
            findings.push(HealthFinding {
                item_id: entry.id,
                name: entry.name,
                subtitle: entry.subtitle,
                issues,
            });
        }
    }

    // Sort high-severity findings (Weak / Reused) first.
    findings.sort_by(|a, b| {
        let a_high = a.issues.iter().filter(|i| i.is_high_severity()).count();
        let b_high = b.issues.iter().filter(|i| i.is_high_severity()).count();
        b_high
            .cmp(&a_high)
            .then_with(|| b.issues.len().cmp(&a.issues.len()))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let score = if scanned_logins == 0 {
        100
    } else {
        let denom = scanned_logins as f32;
        let penalty = (weak_count as f32 / denom) * 45.0
            + (reused_count as f32 / denom) * 35.0
            + (missing_2fa_count as f32 / denom) * 12.0
            + (http_count as f32 / denom) * 8.0
            + (stale_count as f32 / denom) * 6.0;
        (100.0 - penalty).round().clamp(0.0, 100.0) as u8
    };

    HealthReport {
        total_logins,
        scanned_logins,
        skipped_reprompt,
        score,
        weak_count,
        reused_count,
        missing_2fa_count,
        http_count,
        stale_count,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insecure_http_detection_allows_localhost_and_flags_remote_http() {
        assert!(is_insecure_http_uri("http://example.com/login"));
        assert!(is_insecure_http_uri("HTTP://192.168.1.10:8080/admin"));
        assert!(!is_insecure_http_uri("https://example.com/login"));
        assert!(!is_insecure_http_uri("http://localhost:3000"));
        assert!(!is_insecure_http_uri("http://127.0.0.1:8080"));
        assert!(!is_insecure_http_uri("http://[::1]:5173"));
    }

    #[test]
    fn ephemeral_keyed_tags_match_identical_inputs_without_plaintext() {
        let s1 = RandomState::new();
        let s2 = RandomState::new();
        assert_eq!(
            ephemeral_tag(&s1, &s2, "correct-horse-battery"),
            ephemeral_tag(&s1, &s2, "correct-horse-battery")
        );
        assert_ne!(
            ephemeral_tag(&s1, &s2, "correct-horse-battery"),
            ephemeral_tag(&s1, &s2, "different-password")
        );
    }
}
