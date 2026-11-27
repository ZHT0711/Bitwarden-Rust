//! Account timeout policy and one central scheduler. No vault-row timers.
mod host;

use std::{collections::BTreeMap, fs, time::Duration};

use bytewarden_core::desktop::{self, Account, AccountTimeout, Session};
use gpui::{AppContext, Context, Entity, Task, Window};

use crate::{
    VaultPreview,
    client::{profile_directory, status_text},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    AppRestart,
    Inactivity,
    SystemIdle,
    SystemLock,
    SystemSleep,
    Never,
}
impl Mode {
    pub const CHOICES: [(Self, &'static str); 6] = [
        (Self::Inactivity, "经过指定分钟"),
        (Self::SystemIdle, "系统空闲 5 分钟"),
        (Self::SystemLock, "系统锁定时"),
        (Self::SystemSleep, "系统休眠时"),
        (Self::AppRestart, "应用重启时"),
        (Self::Never, "从不"),
    ];
    fn key(self) -> &'static str {
        match self {
            Self::AppRestart => "restart",
            Self::Inactivity => "inactivity",
            Self::SystemIdle => "idle",
            Self::SystemLock => "lock",
            Self::SystemSleep => "sleep",
            Self::Never => "never",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Lock,
    LogOut,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub mode: Mode,
    pub minutes: u32,
    pub action: Action,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            mode: Mode::AppRestart,
            minutes: 15,
            action: Action::Lock,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Capabilities {
    pub lock: bool,
    pub sleep: bool,
    pub idle: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    Lock,
    Sleep,
}
impl Policy {
    pub fn valid(self) -> bool {
        (1..=i32::MAX as u32).contains(&self.minutes)
    }
    pub fn effective_mode(self, caps: Capabilities) -> Mode {
        match self.mode {
            Mode::SystemLock if !caps.lock => {
                if caps.sleep {
                    Mode::SystemSleep
                } else {
                    Mode::AppRestart
                }
            }
            Mode::SystemSleep if !caps.sleep => Mode::AppRestart,
            _ => self.mode,
        }
    }
    fn can_apply(self, state: Session) -> bool {
        state == Session::Unlocked || (state == Session::Locked && self.action == Action::LogOut)
    }
    fn expired(
        self,
        mode: Mode,
        state: Session,
        foreground: bool,
        elapsed: u64,
        idle: Option<u64>,
    ) -> bool {
        self.valid()
            && self.can_apply(state)
            && match mode {
                Mode::Inactivity => {
                    state == Session::Unlocked && !foreground && elapsed >= self.duration()
                }
                Mode::SystemIdle => idle.is_some_and(|idle| idle >= 300_000),
                _ => false,
            }
    }
    fn duration(self) -> u64 {
        u64::from(self.minutes) * 60_000
    }
    fn encode(self) -> String {
        format!(
            "{}\n{}\n{}\n",
            self.mode.key(),
            self.minutes,
            if self.action == Action::LogOut {
                "logout"
            } else {
                "lock"
            }
        )
    }
    fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let key = lines.next()?;
        let mode = Mode::CHOICES.iter().find(|(mode, _)| mode.key() == key)?.0;
        let minutes = lines.next()?.parse().ok()?;
        let action = match lines.next()? {
            "lock" => Action::Lock,
            "logout" => Action::LogOut,
            _ => return None,
        };
        let policy = Self {
            mode,
            minutes,
            action,
        };
        (policy.valid() && lines.next().is_none()).then_some(policy)
    }
    fn load(scope: &str) -> Self {
        profile_directory()
            .ok()
            .and_then(|root| fs::read_to_string(root.join("timeouts").join(scope)).ok())
            .and_then(|text| Self::parse(&text))
            .unwrap_or_default()
    }
    fn save(self, scope: &str) -> std::io::Result<()> {
        let root = profile_directory()?.join("timeouts");
        fs::create_dir_all(&root)?;
        fs::write(root.join(scope), self.encode())
    }
}
struct Activity {
    scope: String,
    policy: Policy,
    last_activity: u64,
    state: Session,
}

#[derive(Clone, Copy)]
struct AccountState<'a> {
    id: &'a str,
    state: Session,
    active: bool,
}
fn account_states(accounts: &[Account]) -> Vec<AccountState<'_>> {
    accounts
        .iter()
        .map(|account| AccountState {
            id: &account.id,
            state: account.session(),
            active: account.active,
        })
        .collect()
}

#[derive(Default)]
pub struct Timeouts {
    accounts: BTreeMap<String, Activity>,
    pending: BTreeMap<String, Action>,
    events: Vec<Event>,
    restarted: bool,
    foreground: bool,
    timer: Option<Task<()>>,
    deadline: Option<u64>,
    host: Option<host::Host>,
    pub never_confirmation: bool,
    pub minutes_input: Option<Entity<crate::input::Input>>,
    draft_account: Option<String>,
    retry_after: u64,
    in_flight: BTreeMap<String, Action>,
}
impl Timeouts {
    pub fn capabilities(&self) -> Capabilities {
        self.host.as_ref().map_or_else(
            || Capabilities {
                idle: cfg!(windows),
                ..Capabilities::default()
            },
            host::Host::capabilities,
        )
    }
    pub fn policy(&self, account: Option<&Account>) -> Policy {
        account
            .and_then(|a| self.accounts.get(&a.id))
            .map_or_else(Policy::default, |a| a.policy)
    }
    fn should_revoke_active(&self, active_id: Option<&str>, caps: Capabilities) -> bool {
        active_id.is_some_and(|id| {
            self.pending.contains_key(id)
                || self.accounts.get(id).is_some_and(|activity| {
                    let mode = activity.policy.effective_mode(caps);
                    self.events.iter().any(|event| {
                        matches!(
                            (event, mode),
                            (Event::Lock, Mode::SystemLock) | (Event::Sleep, Mode::SystemSleep)
                        )
                    })
                })
        })
    }
    fn retain(&mut self, accounts: &[Account], now: u64) {
        self.accounts
            .retain(|id, _| accounts.iter().any(|a| a.id == *id));
        self.pending.retain(|id, action| {
            accounts.iter().any(|account| {
                account.id == *id
                    && (*action == Action::LogOut || account.session() == Session::Unlocked)
            })
        });
        for account in accounts {
            if let Some(activity) = self.accounts.get_mut(&account.id) {
                if activity.state != Session::Unlocked && account.session() == Session::Unlocked {
                    activity.last_activity = now;
                }
                activity.state = account.session();
                continue;
            }
            let Ok(scope) = account.settings_scope() else {
                continue;
            };
            self.accounts.insert(
                account.id.clone(),
                Activity {
                    policy: Policy::load(&scope),
                    scope,
                    last_activity: now,
                    state: account.session(),
                },
            );
        }
    }
    fn finish_transitions(&mut self, accounts: &[AccountState<'_>]) {
        if self.in_flight.is_empty() {
            return;
        }
        let mut failed = false;
        for (id, action) in std::mem::take(&mut self.in_flight) {
            let remains = accounts
                .iter()
                .find(|account| account.id == id)
                .is_some_and(|account| {
                    action == Action::LogOut || account.state == Session::Unlocked
                });
            if remains {
                self.pending.insert(id, action);
                failed = true;
            } else {
                self.pending.remove(&id);
            }
        }
        if !failed {
            self.retry_after = 0;
        }
    }
    fn observe_windows_input(&mut self, active_id: Option<&str>, now: u64, idle: Option<u64>) {
        // Windows input covers IME/native dialogs and consumed wheel events.
        // Never bridge an already-expired interval with input from another app.
        if !self.foreground {
            return;
        }
        let Some(activity) = active_id.and_then(|id| self.accounts.get_mut(id)) else {
            return;
        };
        let Some(observed) = idle.map(|idle| now.saturating_sub(idle)) else {
            return;
        };
        if activity.state == Session::Unlocked
            && observed > activity.last_activity
            && observed.saturating_sub(activity.last_activity) < activity.policy.duration()
        {
            activity.last_activity = observed;
        }
    }
    fn collect(
        &mut self,
        accounts: &[AccountState<'_>],
        now: u64,
        idle: Option<u64>,
        caps: Capabilities,
        ready: bool,
    ) {
        for account in accounts {
            let Some(activity) = self.accounts.get(account.id) else {
                continue;
            };
            let policy = activity.policy;
            let mode = policy.effective_mode(caps);
            let system_event = self.events.iter().any(|event| {
                matches!(
                    (event, mode),
                    (Event::Lock, Mode::SystemLock) | (Event::Sleep, Mode::SystemSleep)
                )
            });
            if (system_event && policy.can_apply(account.state))
                || policy.expired(
                    mode,
                    account.state,
                    account.active && self.foreground,
                    now.saturating_sub(activity.last_activity),
                    idle,
                )
                || (!self.restarted && mode == Mode::AppRestart && policy.action == Action::LogOut)
            {
                self.pending.insert(account.id.to_owned(), policy.action);
            }
        }
        self.restarted = true;
        // A worker may still finish an unlock or create an account after this event.
        // Re-evaluate the event against its final snapshot before consuming it.
        if ready {
            self.events.clear();
        }
    }
    fn next_deadline(
        &self,
        accounts: &[AccountState<'_>],
        now: u64,
        caps: Capabilities,
    ) -> Option<u64> {
        let account_deadline = accounts
            .iter()
            .filter_map(|account| {
                let activity = self.accounts.get(account.id)?;
                let policy = activity.policy;
                if !policy.can_apply(account.state) {
                    return None;
                }
                match policy.effective_mode(caps) {
                    Mode::Inactivity
                        if account.state == Session::Unlocked
                            && !(account.active && self.foreground) =>
                    {
                        let deadline = activity.last_activity.saturating_add(policy.duration());
                        Some(if deadline <= now {
                            now.saturating_add(10_000)
                        } else {
                            deadline
                        })
                    }
                    Mode::SystemIdle if caps.idle => Some(now.saturating_add(30_000)),
                    _ => None,
                }
            })
            .min();
        let retry_deadline =
            (!self.pending.is_empty()).then(|| self.retry_after.max(now.saturating_add(1)));
        account_deadline.into_iter().chain(retry_deadline).min()
    }
}

impl VaultPreview {
    pub fn initialize_timeouts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.timeouts.foreground = window.is_window_active();
        self.timeouts.minutes_input =
            Some(cx.new(|cx| crate::input::Input::new("分钟（1 至 2147483647）", false, cx)));
        let weak = cx.weak_entity();
        let app = cx.to_async();
        let executor = cx.foreground_executor().clone();
        self.timeouts.host = host::Host::new(window, move |event| {
            let weak = weak.clone();
            let mut app = app.clone();
            // A window procedure must never synchronously reenter a borrowed GPUI entity.
            executor
                .spawn(async move {
                    let _ = weak.update(&mut app, |this, cx| this.on_timeout_event(event, cx));
                })
                .detach();
        });
        cx.observe_window_activation(window, |this, window, cx| {
            // Expire the time spent away before recording the new activation.
            this.update_vault_timeouts(cx);
            this.timeouts.foreground = window.is_window_active();
            this.update_vault_timeouts(cx);
        })
        .detach();
    }

    fn on_timeout_event(&mut self, event: Event, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        if !self.timeouts.events.contains(&event) {
            self.timeouts.events.push(event);
        }
        // Evaluate the active account's policy before canceling its worker.
        // Events stay latched while busy so a matching unlock/sign-in is still
        // revoked against the final account snapshot.
        self.update_vault_timeouts(cx);
    }

    pub fn record_vault_activity(&mut self, _cx: &mut Context<Self>) {
        if self.closing || !self.timeouts.foreground {
            return;
        }
        if let Some(activity) = self
            .client
            .active()
            .and_then(|a| self.timeouts.accounts.get_mut(&a.id))
        {
            let now = host::now();
            if activity.state == Session::Unlocked
                && now.saturating_sub(activity.last_activity) >= 1_000
            {
                activity.last_activity = now;
            }
        }
        // Foreground inactivity is suppressed. The existing deadline rechecks its
        // timestamp on wake, so input never allocates timers or requests a redraw.
    }

    pub fn update_vault_timeouts(&mut self, cx: &mut Context<Self>) {
        if self.closing || !self.client.initialized {
            return;
        }
        let now = host::now();
        self.timeouts.retain(&self.client.accounts, now);
        if self.client.ready() {
            self.timeouts
                .finish_transitions(&account_states(&self.client.accounts));
        }
        let active_id = self.client.active().map(|a| a.id.clone());
        if self.timeouts.draft_account != active_id {
            self.timeouts.draft_account = active_id;
            self.timeouts.never_confirmation = false;
            let minutes = self
                .timeouts
                .policy(self.client.active())
                .minutes
                .to_string();
            if let Some(input) = &self.timeouts.minutes_input {
                input.update(cx, |input, cx| input.set(&minutes, cx));
            }
        }
        let idle = host::idle_millis();
        self.timeouts.observe_windows_input(
            self.client.active().map(|account| account.id.as_str()),
            now,
            idle,
        );
        self.timeouts.collect(
            &account_states(&self.client.accounts),
            now,
            idle,
            self.timeouts.capabilities(),
            self.client.ready(),
        );
        // Keep system events latched during a worker operation and immediately hide
        // the active account when its policy matches, including an in-flight unlock.
        let active_target = self.timeouts.should_revoke_active(
            self.client.active().map(|account| account.id.as_str()),
            self.timeouts.capabilities(),
        );
        if active_target {
            self.clear_account_view(cx);
            self.cancel_pending(cx);
        }
        if self.client.ready()
            && now >= self.timeouts.retry_after
            && !self.timeouts.pending.is_empty()
        {
            // Failed storage/core transitions must not create a completion/retry loop.
            self.timeouts.retry_after = now.saturating_add(10_000);
            if let Some((id, action)) = self.client.active().and_then(|a| {
                self.timeouts
                    .pending
                    .remove(&a.id)
                    .map(|action| (a.id.clone(), action))
            }) {
                self.timeouts.in_flight.insert(id, action);
                // Existing transitions own immediate view/clipboard/key revocation.
                match action {
                    Action::Lock => self.request_lock(cx),
                    Action::LogOut => self.request_logout(cx),
                }
            } else {
                self.timeouts.in_flight = std::mem::take(&mut self.timeouts.pending);
                let actions: Vec<_> = self
                    .timeouts
                    .in_flight
                    .iter()
                    .map(|(id, action)| AccountTimeout {
                        id: id.clone(),
                        log_out: *action == Action::LogOut,
                    })
                    .collect();
                self.run(
                    "正在应用账户超时",
                    None,
                    cx,
                    move || {
                        (
                            desktop::apply_timeouts(&actions, false),
                            desktop::accounts(),
                        )
                    },
                    |this, (result, accounts), _| {
                        if let Ok(accounts) = accounts {
                            this.client.accounts = accounts;
                        }
                        if let Err(error) = result {
                            this.client.notice = status_text(&error);
                        }
                    },
                );
            }
        }
        let deadline = if self.client.busy.is_some() && !self.timeouts.pending.is_empty() {
            None
        } else {
            self.timeouts.next_deadline(
                &account_states(&self.client.accounts),
                now,
                self.timeouts.capabilities(),
            )
        };
        if deadline == self.timeouts.deadline {
            return;
        }
        self.timeouts.timer = None;
        self.timeouts.deadline = deadline;
        if let Some(deadline) = deadline {
            let clock = cx.background_executor().clone();
            self.timeouts.timer = Some(cx.spawn(async move |view, app| {
                clock
                    .timer(Duration::from_millis(deadline.saturating_sub(host::now())))
                    .await;
                let _ = view.update(app, |this, cx| {
                    this.timeouts.deadline = None;
                    this.update_vault_timeouts(cx);
                });
            }));
        }
    }

    pub fn save_timeout_policy(&mut self, policy: Policy, cx: &mut Context<Self>) {
        if !policy.valid() || !self.client.ready() || self.client.state() != Session::Unlocked {
            return;
        }
        let Some(activity) = self
            .client
            .active()
            .and_then(|a| self.timeouts.accounts.get_mut(&a.id))
        else {
            return;
        };
        activity.policy = policy;
        activity.last_activity = host::now();
        let saved = policy.save(&activity.scope).is_ok();
        self.timeouts.never_confirmation = false;
        if let Some(input) = &self.timeouts.minutes_input {
            input.update(cx, |input, cx| input.set(&policy.minutes.to_string(), cx));
        }
        self.update_vault_timeouts(cx);
        self.client.notice = if saved {
            "已保存当前账户的超时设置。"
        } else {
            "设置已应用于本次会话，保存到本机失败。"
        }
        .into();
        cx.notify();
    }

    pub fn stop_timeouts(&mut self) {
        self.timeouts.timer = None;
        self.timeouts.deadline = None;
        self.timeouts.host = None;
    }
}

#[cfg(all(test, feature = "test-support"))]
#[path = "timeout_workflow_tests.rs"]
mod workflow_tests;

#[cfg(test)]
mod tests {
    use super::*;
    fn inactivity(action: Action) -> Policy {
        Policy {
            mode: Mode::Inactivity,
            minutes: 1,
            action,
        }
    }
    #[test]
    fn inactivity_is_background_only_and_exact_at_deadline() {
        for action in [Action::Lock, Action::LogOut] {
            let policy = inactivity(action);
            assert!(!policy.expired(Mode::Inactivity, Session::Unlocked, false, 59_999, None));
            assert!(policy.expired(Mode::Inactivity, Session::Unlocked, false, 60_000, None));
            assert!(!policy.expired(Mode::Inactivity, Session::Unlocked, true, u64::MAX, None));
            assert!(!policy.expired(Mode::Inactivity, Session::Locked, false, u64::MAX, None));
        }
    }
    #[test]
    fn idle_logout_includes_locked_accounts_and_fails_closed_on_missing_input() {
        let policy = Policy {
            mode: Mode::SystemIdle,
            ..inactivity(Action::LogOut)
        };
        assert!(!policy.expired(Mode::SystemIdle, Session::Locked, false, 0, None));
        assert!(!policy.expired(Mode::SystemIdle, Session::Locked, false, 0, Some(299_999)));
        assert!(policy.expired(Mode::SystemIdle, Session::Locked, true, 0, Some(300_000)));
        assert!(!policy.expired(
            Mode::SystemIdle,
            Session::SignedOut,
            false,
            0,
            Some(u64::MAX)
        ));
    }
    #[test]
    fn unavailable_system_events_fall_back_like_winui() {
        let policy = Policy {
            mode: Mode::SystemLock,
            ..Policy::default()
        };
        assert_eq!(
            policy.effective_mode(Capabilities::default()),
            Mode::AppRestart
        );
        assert_eq!(
            policy.effective_mode(Capabilities {
                sleep: true,
                ..Capabilities::default()
            }),
            Mode::SystemSleep
        );
        assert_eq!(
            policy.effective_mode(Capabilities {
                lock: true,
                ..Capabilities::default()
            }),
            Mode::SystemLock
        );
    }
    #[test]
    fn storage_roundtrips_all_modes_and_rejects_invalid_or_truncated_values() {
        for (mode, _) in Mode::CHOICES {
            for action in [Action::Lock, Action::LogOut] {
                let policy = Policy {
                    mode,
                    minutes: i32::MAX as u32,
                    action,
                };
                assert_eq!(Policy::parse(&policy.encode()), Some(policy));
            }
        }
        for text in [
            "",
            "idle\n15",
            "idle\n0\nlock",
            "idle\n2147483648\nlock",
            "unknown\n15\nlock",
            "restart\n15\nother",
            "never\n15\nlock\nextra",
        ] {
            assert_eq!(Policy::parse(text), None);
        }
    }
    #[test]
    fn independent_accounts_expire_without_affecting_foreground_account() {
        let mut timeouts = Timeouts {
            restarted: true,
            foreground: true,
            ..Timeouts::default()
        };
        for (id, action) in [("active", Action::Lock), ("background", Action::LogOut)] {
            timeouts.accounts.insert(
                id.into(),
                Activity {
                    scope: String::new(),
                    policy: inactivity(action),
                    last_activity: 100,
                    state: Session::Unlocked,
                },
            );
        }
        let accounts = [
            AccountState {
                id: "active",
                state: Session::Unlocked,
                active: true,
            },
            AccountState {
                id: "background",
                state: Session::Unlocked,
                active: false,
            },
        ];
        assert_eq!(
            timeouts.next_deadline(&accounts, 200, Capabilities::default()),
            Some(60_100)
        );
        timeouts.collect(&accounts, 60_100, None, Capabilities::default(), true);
        assert_eq!(timeouts.pending.len(), 1);
        assert_eq!(timeouts.pending.get("background"), Some(&Action::LogOut));
        assert!(!timeouts.pending.contains_key("active"));
    }
    #[test]
    fn system_events_revoke_only_the_matching_active_account_worker() {
        let caps = Capabilities {
            lock: true,
            sleep: true,
            idle: true,
        };
        let mut timeouts = Timeouts {
            restarted: true,
            ..Timeouts::default()
        };
        for (id, mode) in [("active", Mode::Never), ("background", Mode::SystemLock)] {
            timeouts.accounts.insert(
                id.into(),
                Activity {
                    scope: String::new(),
                    policy: Policy {
                        mode,
                        ..Policy::default()
                    },
                    last_activity: 100,
                    state: Session::Unlocked,
                },
            );
        }
        let accounts = [
            AccountState {
                id: "active",
                state: Session::Unlocked,
                active: true,
            },
            AccountState {
                id: "background",
                state: Session::Unlocked,
                active: false,
            },
        ];
        timeouts.events.push(Event::Lock);
        timeouts.collect(&accounts, 200, None, caps, false);
        assert_eq!(
            timeouts
                .pending
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["background"]
        );
        assert!(!timeouts.should_revoke_active(Some("active"), caps));
        assert!(timeouts.should_revoke_active(Some("background"), caps));
        assert!(!timeouts.should_revoke_active(None, caps));
        assert_eq!(timeouts.events, vec![Event::Lock]);

        // The same event must still interrupt an active unlock even before that
        // account has reached Unlocked and become a collected timeout target.
        timeouts.pending.clear();
        let activity = timeouts.accounts.get_mut("background").unwrap();
        activity.state = Session::Locked;
        assert!(timeouts.should_revoke_active(Some("background"), caps));
        timeouts.accounts.get_mut("background").unwrap().policy.mode = Mode::SystemSleep;
        assert!(!timeouts.should_revoke_active(Some("background"), caps));
        timeouts.events = vec![Event::Sleep];
        assert!(timeouts.should_revoke_active(Some("background"), caps));
        assert!(!timeouts.should_revoke_active(Some("active"), caps));
    }

    #[test]
    fn lock_event_survives_busy_unlock_and_sleep_targets_only_matching_policies() {
        let caps = Capabilities {
            lock: true,
            sleep: true,
            idle: true,
        };
        let mut timeouts = Timeouts {
            restarted: true,
            ..Timeouts::default()
        };
        for (id, mode, action) in [
            ("lock", Mode::SystemLock, Action::Lock),
            ("sleep", Mode::SystemSleep, Action::LogOut),
            ("never", Mode::Never, Action::LogOut),
        ] {
            timeouts.accounts.insert(
                id.into(),
                Activity {
                    scope: String::new(),
                    policy: Policy {
                        mode,
                        action,
                        ..Policy::default()
                    },
                    last_activity: 100,
                    state: Session::Locked,
                },
            );
        }
        timeouts.events.push(Event::Lock);
        let locked = [AccountState {
            id: "lock",
            state: Session::Locked,
            active: true,
        }];
        timeouts.collect(&locked, 200, None, caps, false);
        assert!(timeouts.pending.is_empty());
        assert_eq!(timeouts.events, vec![Event::Lock]);
        let unlocked = [AccountState {
            state: Session::Unlocked,
            ..locked[0]
        }];
        timeouts.collect(&unlocked, 250, None, caps, true);
        assert_eq!(timeouts.pending.remove("lock"), Some(Action::Lock));
        assert!(timeouts.events.is_empty());
        timeouts.events.push(Event::Sleep);
        let accounts = [
            unlocked[0],
            AccountState {
                id: "sleep",
                state: Session::Locked,
                active: false,
            },
            AccountState {
                id: "never",
                state: Session::Unlocked,
                active: false,
            },
        ];
        timeouts.collect(&accounts, 300, None, caps, true);
        assert_eq!(timeouts.pending.len(), 1);
        assert_eq!(timeouts.pending.get("sleep"), Some(&Action::LogOut));
    }
    #[test]
    fn restart_logout_is_once_and_idle_does_not_allocate_per_account_timers() {
        let mut timeouts = Timeouts::default();
        for id in ["first", "second"] {
            timeouts.accounts.insert(
                id.into(),
                Activity {
                    scope: String::new(),
                    policy: Policy {
                        action: Action::LogOut,
                        ..Policy::default()
                    },
                    last_activity: 0,
                    state: Session::Locked,
                },
            );
        }
        let accounts = [
            AccountState {
                id: "first",
                state: Session::Locked,
                active: true,
            },
            AccountState {
                id: "second",
                state: Session::Locked,
                active: false,
            },
        ];
        timeouts.collect(&accounts, 0, None, Capabilities::default(), true);
        assert_eq!(timeouts.pending.len(), 2);
        timeouts.pending.clear();
        timeouts.collect(&accounts, 1, None, Capabilities::default(), true);
        assert!(timeouts.pending.is_empty());
        assert_eq!(
            timeouts.next_deadline(&accounts, 1, Capabilities::default()),
            None
        );
        for activity in timeouts.accounts.values_mut() {
            activity.policy.mode = Mode::SystemIdle;
        }
        assert_eq!(
            timeouts.next_deadline(
                &accounts,
                1,
                Capabilities {
                    idle: true,
                    ..Capabilities::default()
                }
            ),
            Some(30_001)
        );
    }
    #[test]
    fn successful_targets_clear_stale_pending_and_failed_targets_keep_backoff() {
        let mut timeouts = Timeouts {
            retry_after: 10_000,
            ..Timeouts::default()
        };
        for (id, action) in [
            ("locked", Action::Lock),
            ("failed", Action::LogOut),
            ("gone", Action::LogOut),
        ] {
            timeouts.in_flight.insert(id.into(), action);
            timeouts.pending.insert(id.into(), action);
        }
        let accounts = [
            AccountState {
                id: "locked",
                state: Session::Locked,
                active: true,
            },
            AccountState {
                id: "failed",
                state: Session::Locked,
                active: false,
            },
        ];
        timeouts.finish_transitions(&accounts);
        assert_eq!(timeouts.pending.len(), 1);
        assert_eq!(timeouts.pending.get("failed"), Some(&Action::LogOut));
        assert_eq!(timeouts.retry_after, 10_000);
        timeouts.finish_transitions(&accounts);
        assert_eq!(timeouts.retry_after, 10_000);
        timeouts.in_flight.insert("failed".into(), Action::LogOut);
        timeouts.finish_transitions(&accounts[..1]);
        assert!(timeouts.pending.is_empty());
        assert_eq!(timeouts.retry_after, 0);
    }
    #[test]
    fn windows_input_does_not_bridge_expired_or_background_intervals() {
        let mut timeouts = Timeouts {
            foreground: true,
            ..Timeouts::default()
        };
        timeouts.accounts.insert(
            "active".into(),
            Activity {
                scope: String::new(),
                policy: inactivity(Action::Lock),
                last_activity: 1_000,
                state: Session::Unlocked,
            },
        );
        timeouts.observe_windows_input(Some("active"), 30_000, Some(1_000));
        assert_eq!(timeouts.accounts["active"].last_activity, 29_000);
        timeouts.observe_windows_input(Some("active"), 90_000, Some(500));
        assert_eq!(timeouts.accounts["active"].last_activity, 29_000);
        timeouts.foreground = false;
        timeouts.observe_windows_input(Some("active"), 40_000, Some(0));
        assert_eq!(timeouts.accounts["active"].last_activity, 29_000);
    }
    #[test]
    fn retry_backoff_and_regressed_clock_do_not_spin_or_expire_early() {
        let mut timeouts = Timeouts {
            restarted: true,
            retry_after: 10_000,
            ..Timeouts::default()
        };
        timeouts.accounts.insert(
            "account".into(),
            Activity {
                scope: String::new(),
                policy: inactivity(Action::Lock),
                last_activity: 10_000,
                state: Session::Unlocked,
            },
        );
        let accounts = [AccountState {
            id: "account",
            state: Session::Unlocked,
            active: false,
        }];
        timeouts.collect(&accounts, 5_000, None, Capabilities::default(), true);
        assert!(timeouts.pending.is_empty());
        timeouts.pending.insert("account".into(), Action::Lock);
        assert_eq!(
            timeouts.next_deadline(&accounts, 5_000, Capabilities::default()),
            Some(10_000)
        );
    }
    #[test]
    fn large_elapsed_suspend_time_expires_without_overflow() {
        let policy = Policy {
            minutes: i32::MAX as u32,
            ..inactivity(Action::Lock)
        };
        assert!(!policy.expired(
            Mode::Inactivity,
            Session::Unlocked,
            false,
            policy.duration() - 1,
            None
        ));
        assert!(policy.expired(Mode::Inactivity, Session::Unlocked, false, u64::MAX, None));
        for mode in [
            Mode::Never,
            Mode::AppRestart,
            Mode::SystemLock,
            Mode::SystemSleep,
        ] {
            assert!(!policy.expired(mode, Session::Unlocked, false, u64::MAX, Some(u64::MAX)));
        }
    }
}
