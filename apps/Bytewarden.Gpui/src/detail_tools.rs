use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bytewarden_core::desktop::{Detail, Session, Totp, TotpCode};
use gpui::{Context, PathPromptOptions, Subscription, Task};
use zeroize::Zeroizing;

use crate::{VaultPreview, client::status_text};

fn unix_time() -> Option<Duration> {
    SystemTime::now().duration_since(UNIX_EPOCH).ok()
}

#[derive(Debug, PartialEq, Eq)]
enum TotpTiming {
    Current { seconds: u64, boundary: Duration },
    Refresh,
    Unavailable,
}

impl TotpTiming {
    fn at(value: &TotpCode, now: Option<Duration>) -> Self {
        let Some(now) = now else {
            return Self::Unavailable;
        };
        let (Ok(from), Ok(until)) = (
            u64::try_from(value.valid_from),
            u64::try_from(value.valid_until),
        ) else {
            return Self::Unavailable;
        };
        if value.code.is_empty()
            || value.period == 0
            || until.checked_sub(from) != Some(u64::from(value.period))
        {
            return Self::Unavailable;
        }
        if now < Duration::from_secs(from) || now >= Duration::from_secs(until) {
            return Self::Refresh;
        }
        Self::Current {
            seconds: until - now.as_secs(),
            boundary: Duration::from_secs(until) - now,
        }
    }
}

struct TotpScope<'a> {
    request_id: Option<u64>,
    account: Option<&'a str>,
    allowed: bool,
}

struct TotpIdentity {
    request_id: u64,
    account: String,
}

impl TotpIdentity {
    fn matches(&self, scope: TotpScope<'_>) -> bool {
        scope.allowed
            && scope.request_id == Some(self.request_id)
            && scope.account == Some(self.account.as_str())
    }
}

// The notification subscription owns this watch. Invalidating the detail or
// releasing its parent drops both timers and both subscriptions, breaking that
// ownership cycle. The timer futures retain only weak references.
struct TotpWatch {
    identity: TotpIdentity,
    boundary: Option<Task<()>>,
    countdown: Option<Task<()>>,
    observation: Option<Subscription>,
    release: Option<Subscription>,
    refreshing: bool,
    stopped: bool,
}

impl TotpWatch {
    fn stop(&mut self) {
        self.stopped = true;
        self.refreshing = false;
        self.boundary = None;
        self.countdown = None;
        self.observation = None;
        self.release = None;
    }
}

fn schedule_totp_boundary(
    watch: &Rc<RefCell<TotpWatch>>,
    delay: Duration,
    cx: &mut Context<VaultPreview>,
) {
    let state = Rc::downgrade(watch);
    let clock = cx.background_executor().clone();
    watch.borrow_mut().boundary = Some(cx.spawn(async move |weak, app| {
        clock.timer(delay).await;
        let Some(watch) = state.upgrade() else {
            return;
        };
        let lease = weak
            .update(app, |this, cx| {
                if !watch.borrow().identity.matches(this.totp_scope()) {
                    watch.borrow_mut().stop();
                    return None;
                }
                let detail = this.client.detail.as_ref()?;
                let Some(Totp::Ready { value }) = detail.totp.as_ref() else {
                    return None;
                };
                match TotpTiming::at(value, unix_time()) {
                    TotpTiming::Current { boundary, .. } => {
                        // Re-evaluate wall time after sleep/resume or a clock correction.
                        schedule_totp_boundary(&watch, boundary, cx);
                        None
                    }
                    TotpTiming::Refresh if !this.client.ready() => None,
                    TotpTiming::Refresh => {
                        watch.borrow_mut().refreshing = true;
                        cx.notify();
                        detail.totp_lease()
                    }
                    TotpTiming::Unavailable => {
                        watch.borrow_mut().stop();
                        cx.notify();
                        None
                    }
                }
            })
            .ok()
            .flatten();
        let Some(lease) = lease else {
            return;
        };
        let result = clock.spawn(async move { lease.refresh() }).await;
        let _ = weak.update(app, |this, cx| {
            if watch.borrow().stopped || !watch.borrow().identity.matches(this.totp_scope()) {
                watch.borrow_mut().stop();
                return;
            }
            watch.borrow_mut().refreshing = false;
            match result {
                Ok(value) => {
                    let timing = TotpTiming::at(&value, unix_time());
                    if let TotpTiming::Current { boundary, .. } = timing {
                        if let Some(detail) = this.client.detail.as_mut() {
                            detail.totp = Some(Totp::Ready { value });
                        }
                        schedule_totp_boundary(&watch, boundary, cx);
                    } else {
                        // Never retry an invalid window in a tight refresh loop.
                        if let Some(detail) = this.client.detail.as_mut() {
                            detail.totp = None;
                        }
                        this.client.notice = "系统时间已变化，请重新打开条目查看验证码。".into();
                        watch.borrow_mut().stop();
                    }
                }
                Err(error) => {
                    if let Some(detail) = this.client.detail.as_mut() {
                        detail.totp = None;
                    }
                    this.client.notice =
                        format!("{} 请重新打开条目查看验证码。", status_text(&error));
                    watch.borrow_mut().stop();
                }
            }
            cx.notify();
        });
    }));
}

#[cfg(test)]
#[path = "detail_otp_tests.rs"]
mod tests;

pub(crate) struct TotpDisplayState {
    pub code: Option<String>,
    pub remaining_secs: Option<u64>,
    pub fraction: f32,
    pub urgent: bool,
    pub can_copy: bool,
    pub status_message: String,
}

#[derive(Clone, Copy)]
pub enum FileChooser {
    Open,
    Save,
}

pub fn choose_file(
    cx: &mut Context<VaultPreview>,
    generation: u64,
    chooser: FileChooser,
    name: Option<&str>,
    on_path: impl FnOnce(&mut VaultPreview, PathBuf, &mut Context<VaultPreview>) + 'static,
) {
    let open = || PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: None,
    };
    let open = matches!(chooser, FileChooser::Open).then(|| cx.prompt_for_paths(open()));
    let save = if open.is_none() {
        let directory = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let basename = name
            .and_then(|value| std::path::Path::new(value).file_name())
            .and_then(|value| value.to_str());
        Some(cx.prompt_for_new_path(&directory, basename))
    } else {
        None
    };
    cx.spawn(async move |weak, app| {
        let path = if let Some(open) = open {
            open.await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|paths| paths.into_iter().next())
        } else if let Some(save) = save {
            save.await.ok().and_then(Result::ok).flatten()
        } else {
            None
        };
        if let Some(path) = path {
            let _ = weak.update(app, |this, cx| {
                if this.client.generation == generation && this.client.ready() && !this.lock_queued
                {
                    on_path(this, path, cx);
                }
            });
        }
    })
    .detach();
}

impl VaultPreview {
    fn totp_scope(&self) -> TotpScope<'_> {
        let detail = self.client.detail.as_ref();
        TotpScope {
            request_id: detail.and_then(|detail| detail.totp_request_id),
            account: self.client.active().map(|account| account.id.as_str()),
            allowed: !self.lock_queued
                && !self.closing
                && self.client.state() == Session::Unlocked
                && self.client.overview.is_some()
                && detail.is_some_and(|detail| {
                    !detail.restricted
                        && !detail.password_hidden
                        && matches!(detail.totp, Some(Totp::Ready { .. }))
                }),
        }
    }

    pub fn watch_totp(&self, cx: &mut Context<Self>) {
        let scope = self.totp_scope();
        let (Some(request_id), Some(account)) = (scope.request_id, scope.account) else {
            return;
        };
        if !scope.allowed {
            return;
        }
        let watch = Rc::new(RefCell::new(TotpWatch {
            identity: TotpIdentity {
                request_id,
                account: account.to_owned(),
            },
            boundary: None,
            countdown: None,
            observation: None,
            release: None,
            refreshing: false,
            stopped: false,
        }));
        let observed = watch.clone();
        watch.borrow_mut().observation = Some(cx.observe_self(move |this, cx| {
            if !observed.borrow().identity.matches(this.totp_scope()) {
                observed.borrow_mut().stop();
                return;
            }
            // An unrelated detail operation can occupy the core at the boundary.
            // Resume from its completion notification instead of retry polling.
            if this.client.ready()
                && !observed.borrow().refreshing
                && this.client.detail.as_ref().is_some_and(|detail| {
                    matches!(
                        detail.totp.as_ref(),
                        Some(Totp::Ready { value })
                            if TotpTiming::at(value, unix_time()) == TotpTiming::Refresh
                    )
                })
            {
                schedule_totp_boundary(&observed, Duration::ZERO, cx);
            }
        }));
        let released = Rc::downgrade(&watch);
        watch.borrow_mut().release = Some(cx.on_release(move |_, _| {
            if let Some(watch) = released.upgrade() {
                watch.borrow_mut().stop();
            }
        }));

        let timing = self
            .client
            .detail
            .as_ref()
            .and_then(|detail| detail.totp.as_ref())
            .and_then(|totp| match totp {
                Totp::Ready { value } => Some(TotpTiming::at(value, unix_time())),
                _ => None,
            });
        let delay = match timing {
            Some(TotpTiming::Current { boundary, .. }) => boundary,
            Some(TotpTiming::Refresh) => Duration::ZERO,
            _ => {
                watch.borrow_mut().stop();
                return;
            }
        };
        schedule_totp_boundary(&watch, delay, cx);

        let state = Rc::downgrade(&watch);
        let clock = cx.background_executor().clone();
        watch.borrow_mut().countdown = Some(cx.spawn(async move |weak, app| {
            loop {
                let next_second = unix_time().map_or(Duration::from_secs(1), |now| {
                    Duration::from_secs(1) - Duration::from_nanos(u64::from(now.subsec_nanos()))
                });
                clock.timer(next_second).await;
                let Some(watch) = state.upgrade() else {
                    break;
                };
                let active = weak
                    .update(app, |this, cx| {
                        if watch.borrow().stopped
                            || !watch.borrow().identity.matches(this.totp_scope())
                        {
                            watch.borrow_mut().stop();
                            return false;
                        }
                        let timing = this
                            .client
                            .detail
                            .as_ref()
                            .and_then(|detail| detail.totp.as_ref())
                            .and_then(|totp| match totp {
                                Totp::Ready { value } => Some(TotpTiming::at(value, unix_time())),
                                _ => None,
                            });
                        if matches!(timing, Some(TotpTiming::Unavailable) | None) {
                            watch.borrow_mut().stop();
                        }
                        // Presentation-only tick. Boundary refresh and clock-jump
                        // recovery are handled by the boundary task/observer.
                        cx.notify();
                        !watch.borrow().stopped
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        }));
    }

    fn pick_attachment(&mut self, id: Option<(String, String)>, cx: &mut Context<Self>) {
        if !self.client.ready() {
            return;
        }
        let Some(detail) = self.client.detail.as_ref() else {
            return;
        };
        let Some(lease) = detail.attachment_lease() else {
            return;
        };
        let lease_token = detail.attachment_request_id;
        if id.is_none() && !detail.can_upload_attachments {
            return;
        }
        let chooser = if id.is_some() {
            FileChooser::Save
        } else {
            FileChooser::Open
        };
        let hint = id.as_ref().map(|(_, name)| name.clone());
        choose_file(
            cx,
            self.client.generation,
            chooser,
            hint.as_deref(),
            move |this, path, cx| {
                if this
                    .client
                    .detail
                    .as_ref()
                    .and_then(|d| d.attachment_request_id)
                    != lease_token
                {
                    return;
                }
                let label = if id.is_some() {
                    "正在下载附件"
                } else {
                    "正在上传附件"
                };
                this.run(
                    label,
                    Some(lease.request()),
                    cx,
                    move || {
                        if let Some((id, _)) = id {
                            lease.download(&id, &path).map(|()| None)
                        } else {
                            lease.upload(&path).map(Some)
                        }
                    },
                    |this, result, _| match result {
                        Ok(Some(result)) if !this.lock_queued => {
                            this.close_detail();
                            this.client.set_overview(result.overview);
                            this.invalidate_query();
                            this.client.notice = "附件已上传，请重新打开条目查看。".into();
                        }
                        Ok(None) => this.client.notice = "附件已下载。".into(),
                        Ok(Some(_)) => {}
                        Err(error) => {
                            if matches!(
                                error.code(),
                                "cancelled" | "attachmentViewExpired" | "writeUncertain"
                            ) {
                                this.close_detail();
                            }
                            this.client.notice = status_text(&error);
                        }
                    },
                );
            },
        );
    }

    fn delete_attachment(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.client.ready() {
            return;
        }
        let Some(lease) = self
            .client
            .detail
            .as_ref()
            .filter(|detail| detail.can_delete_attachments)
            .and_then(Detail::attachment_lease)
        else {
            return;
        };
        // ID is selected in an explicit inline confirmation rendered by the detail pane.
        let id = id.to_owned();
        self.confirm_attachment = None;
        self.run(
            "正在删除附件",
            Some(lease.request()),
            cx,
            move || lease.delete(&id),
            |this, result, _| match result {
                Ok(report) if !this.lock_queued => {
                    this.close_detail();
                    this.client.set_overview(report.overview);
                    this.invalidate_query();
                    this.client.notice = "附件已删除。".into();
                }
                Ok(_) => {}
                Err(error) => {
                    this.close_detail();
                    this.client.notice = status_text(&error);
                }
            },
        );
    }

    pub(crate) fn totp_allowed_now(&self) -> bool {
        self.totp_scope().allowed
    }

    pub(crate) fn current_totp_code(&self) -> Option<Zeroizing<String>> {
        let detail = self.client.detail.as_ref()?;
        let Some(Totp::Ready { value }) = detail.totp.as_ref() else {
            return None;
        };
        matches!(
            TotpTiming::at(value, unix_time()),
            TotpTiming::Current { .. }
        )
        .then(|| Zeroizing::new(value.code.clone()))
    }

    pub(crate) fn totp_display_state(&self, detail: &Detail) -> Option<TotpDisplayState> {
        let totp = detail.totp.as_ref()?;
        let busy = !self.client.ready();
        let allowed = self.totp_scope().allowed;
        match totp {
            Totp::Ready { value } if allowed => match TotpTiming::at(value, unix_time()) {
                TotpTiming::Current { seconds, boundary } => {
                    let period = f32::from(value.period.max(1) as u16);
                    let fraction = (boundary.as_secs_f32() / period).clamp(0., 1.);
                    let can_copy = !busy && detail.totp_request_id.is_some();
                    Some(TotpDisplayState {
                        code: Some(value.code.clone()),
                        remaining_secs: Some(seconds),
                        fraction,
                        urgent: seconds <= 5,
                        can_copy,
                        status_message: String::new(),
                    })
                }
                TotpTiming::Refresh => Some(TotpDisplayState {
                    code: None,
                    remaining_secs: None,
                    fraction: 0.,
                    urgent: false,
                    can_copy: false,
                    status_message: "正在更新验证码…".into(),
                }),
                TotpTiming::Unavailable => Some(TotpDisplayState {
                    code: None,
                    remaining_secs: None,
                    fraction: 0.,
                    urgent: false,
                    can_copy: false,
                    status_message: "系统时间不可用，请检查时间并重新打开条目。".into(),
                }),
            },
            Totp::Ready { .. } => Some(TotpDisplayState {
                code: None,
                remaining_secs: None,
                fraction: 0.,
                urgent: false,
                can_copy: false,
                status_message: "请重新打开条目查看验证码。".into(),
            }),
            Totp::PremiumRequired => Some(TotpDisplayState {
                code: None,
                remaining_secs: None,
                fraction: 0.,
                urgent: false,
                can_copy: false,
                status_message: "生成验证码需要个人或组织的高级会员资格。".into(),
            }),
            Totp::Invalid => Some(TotpDisplayState {
                code: None,
                remaining_secs: None,
                fraction: 0.,
                urgent: false,
                can_copy: false,
                status_message: "验证器密钥不可用。".into(),
            }),
        }
    }

    pub(crate) fn pick_attachment_ui(
        &mut self,
        id: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        self.pick_attachment(id, cx);
    }

    pub(crate) fn delete_attachment_ui(&mut self, id: &str, cx: &mut Context<Self>) {
        self.delete_attachment(id, cx);
    }
}
