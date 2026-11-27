//! Desktop presentation state. The shared core owns the session and key material.
use std::{
    fs, io,
    path::{Path, PathBuf},
};

use bytewarden_core::desktop::{
    self, Account, AuthStatus, Detail, Error, ItemAction, Login, Overview, Provider, Providers,
    Request, Server, Session,
};
use gpui::{Context, Entity, Window, div, prelude::*, px};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{PendingQuickCopy, QuickCopyAction, VaultPreview, input::Input};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Us,
    Eu,
    Custom,
}
impl Region {
    fn from_mode(mode: u32) -> Self {
        match mode {
            0 => Self::Us,
            1 => Self::Eu,
            _ => Self::Custom,
        }
    }

    fn server(self, url: &str) -> Server<'_> {
        match self {
            Self::Us => Server::Us,
            Self::Eu => Server::Eu,
            Self::Custom => Server::SelfHosted(url),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Form {
    SignIn,
    Unlock,
    HelloEnrollment,
    PinEnrollment,
    Detail,
    EditorPassword,
    Confirm(ItemAction),
    Reverify(ItemAction),
}

#[derive(Clone, Copy)]
enum Challenge {
    TwoFactor(TwoFactorOptions),
    Device,
}

#[derive(Clone, Copy)]
struct TwoFactorOptions {
    supported: [bool; 3],
    manual_authenticator: bool,
}

impl TwoFactorOptions {
    fn new(region: Region, supported: [bool; 3]) -> Self {
        Self {
            supported,
            // Match WinUI TwoFactorMethods.OffersManualAuthenticator: the server
            // must still accept the submitted token; this only offers an input.
            manual_authenticator: region == Region::Custom && !supported.contains(&true),
        }
    }

    fn from_providers(region: Region, providers: Providers) -> Self {
        Self::new(
            region,
            PROVIDERS.map(|(provider, _)| providers.supports(provider)),
        )
    }

    fn offers(self, provider: Provider) -> bool {
        let index = match provider {
            Provider::Authenticator => 0,
            Provider::Email => 1,
            Provider::YubiKey => 2,
        };
        self.supported[index] || (provider == Provider::Authenticator && self.manual_authenticator)
    }

    fn selected(self, previous: Provider) -> Option<Provider> {
        if self.offers(previous) {
            Some(previous)
        } else {
            PROVIDERS
                .iter()
                .map(|(provider, _)| *provider)
                .find(|provider| self.offers(*provider))
        }
    }
}

#[derive(Default)]
struct LoginChallenge {
    challenge: Option<Challenge>,
    pending_device_otp: Option<Zeroizing<String>>,
    generation: u64,
}

impl LoginChallenge {
    fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    fn reset(&mut self) {
        self.challenge = None;
        self.pending_device_otp = None;
        self.generation = self.generation.wrapping_add(1);
    }

    fn device_code(&mut self, code: &mut Zeroizing<String>) -> Zeroizing<String> {
        match self.challenge {
            Some(Challenge::Device) => std::mem::take(code),
            Some(Challenge::TwoFactor(_)) => self.pending_device_otp.take().unwrap_or_default(),
            None => Zeroizing::default(),
        }
    }

    fn two_factor(&mut self, options: TwoFactorOptions, device_code: Zeroizing<String>) {
        self.pending_device_otp = (!device_code.is_empty()).then_some(device_code);
        self.challenge = Some(Challenge::TwoFactor(options));
    }

    fn device(&mut self) {
        self.pending_device_otp = None;
        self.challenge = Some(Challenge::Device);
    }

    // A discarded device OTP cannot be silently omitted on a two-factor retry.
    // Restart that chain with a visible password field instead.
    fn failure(&mut self, status: Option<AuthStatus>, had_device_code: bool) -> bool {
        let had_device_code = had_device_code || self.pending_device_otp.is_some();
        self.pending_device_otp = None;
        if status == Some(AuthStatus::InvalidDeviceCode) {
            self.device();
            return false;
        }
        let retryable = matches!(
            status,
            Some(AuthStatus::InvalidTwoFactor | AuthStatus::TimedOut | AuthStatus::RateLimited)
        );
        if !retryable || had_device_code {
            self.reset();
            true
        } else {
            false
        }
    }
}

pub struct Client {
    pub accounts: Vec<Account>,
    pub overview: Option<Overview>,
    pub overview_generation: u64,
    pub overview_error: Option<String>,
    pub write_guard: bool,
    pub detail: Option<Detail>,
    pub revealed: std::collections::HashSet<usize>,
    pub(crate) rendered_secrets: crate::sensitive_text::Registry,
    pub editor: Option<crate::editor::Editor>,
    pub form: Option<Form>,
    pub(crate) form_generation: u64,
    reconnect_account: Option<String>,
    local_unlock_pending: bool,
    hello_window: Option<usize>,
    pub notice: String,
    pub busy: Option<&'static str>,
    pub search: Entity<Input>,
    pub generation: u64,
    pub show_accounts: bool,
    pub selected_folder: Option<String>,
    pub(crate) show_folders: bool,
    pub(crate) pending: Option<Request>,
    pub(crate) initialized: bool,
    #[cfg(all(test, feature = "test-support"))]
    initialization_results: std::collections::VecDeque<Result<(Uuid, Snapshot), String>>,
    pub(crate) session_transition_pending: bool,
    region: Region,
    login_challenge: LoginChallenge,
    provider: Provider,
    remember: bool,
    device: Option<Uuid>,
    email: Entity<Input>,
    server: Entity<Input>,
    pub(crate) password: Entity<Input>,
    pub(crate) pin: Entity<Input>,
    pub(crate) pin_require_mp_on_restart: bool,
    pub(crate) unlock_with_pin: bool,
    pub(crate) enroll_hello_on_login: bool,
    code: Entity<Input>,
}

impl Client {
    #[cfg(all(test, windows))]
    pub(crate) fn set_probe_password(&self, cx: &mut Context<VaultPreview>) {
        self.password.update(cx, |field, cx| {
            field.set("synthetic-master-password-memory-probe", cx)
        });
    }

    pub fn new(cx: &mut Context<VaultPreview>) -> Self {
        Self {
            accounts: Vec::new(),
            overview: None,
            overview_generation: 0,
            overview_error: None,
            write_guard: false,
            detail: None,
            revealed: Default::default(),
            rendered_secrets: Default::default(),
            editor: None,
            form: None,
            form_generation: 0,
            reconnect_account: None,
            local_unlock_pending: false,
            hello_window: None,
            notice: "正在初始化本机保险库…".into(),
            busy: None,
            generation: 0,
            show_accounts: false,
            selected_folder: None,
            show_folders: false,
            pending: None,
            initialized: false,
            #[cfg(all(test, feature = "test-support"))]
            initialization_results: Default::default(),
            session_transition_pending: false,
            region: Region::Us,
            login_challenge: LoginChallenge::default(),
            provider: Provider::Authenticator,
            remember: false,
            device: None,
            search: cx.new(|cx| Input::new("搜索保险库", false, cx)),
            email: cx.new(|cx| Input::new("电子邮箱", false, cx)),
            server: cx.new(|cx| Input::new("https://vault.example.com", false, cx)),
            password: cx.new(|cx| Input::new("主密码", true, cx)),
            pin: cx.new(|cx| Input::new("输入 PIN 码（至少 4 位）", true, cx)),
            pin_require_mp_on_restart: false,
            unlock_with_pin: false,
            enroll_hello_on_login: cfg!(all(windows, not(test))),
            code: cx.new(|cx| Input::new("验证码", true, cx)),
        }
    }

    pub fn set_overview(&mut self, overview: Overview) {
        self.overview = Some(overview);
        self.overview_generation = self.overview_generation.wrapping_add(1);
    }

    pub fn clear_overview(&mut self) {
        self.overview = None;
        self.overview_generation = self.overview_generation.wrapping_add(1);
    }

    pub fn active(&self) -> Option<&Account> {
        self.accounts.iter().find(|a| a.active)
    }

    fn reconnect(&self) -> Option<&Account> {
        let id = self.reconnect_account.as_deref()?;
        self.active().filter(|account| account.id == id)
    }

    fn local_unlock_account(&self) -> Option<&Account> {
        match self.form {
            Some(Form::Unlock) => self.active(),
            Some(Form::SignIn) => self.reconnect(),
            _ => None,
        }
        .filter(|account| account.session() == Session::Locked)
    }
    pub fn state(&self) -> Session {
        self.active().map_or(Session::SignedOut, Account::session)
    }
    pub fn sync_available(&self) -> bool {
        self.ready()
            && self.active().is_some_and(|account| {
                account.session() == Session::Unlocked && !account.needs_sign_in
            })
    }

    pub(crate) fn reconnect_available(&self) -> bool {
        self.ready()
            && self.active().is_some_and(|account| {
                account.session() == Session::Unlocked && account.needs_sign_in
            })
    }

    pub fn ready(&self) -> bool {
        self.initialized && self.busy.is_none()
    }

    fn freeze_auth_inputs(&self, read_only: bool, cx: &mut Context<VaultPreview>) {
        for field in [
            &self.email,
            &self.server,
            &self.password,
            &self.pin,
            &self.code,
        ] {
            field.update(cx, |field, cx| field.set_read_only(read_only, cx));
        }
    }
    pub fn redact_detail(&mut self) {
        self.rendered_secrets.clear();
        self.revealed.clear();
        if let Some(detail) = self.detail.take() {
            detail.revoke();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(req) = self.pending.take() {
            req.cancel();
        }
        self.redact_detail();
    }
}

struct Snapshot {
    accounts: Vec<Account>,
    overview: Option<Overview>,
    overview_error: Option<Error>,
}
fn snapshot() -> Result<Snapshot, Error> {
    let accounts = desktop::accounts()?;
    let (overview, overview_error) = if desktop::session() == Session::Unlocked {
        match desktop::overview() {
            Ok(overview) => (Some(overview), None),
            Err(err) if err.code() == "notFound" => (None, None),
            Err(err) => (None, Some(err)),
        }
    } else {
        (None, None)
    };
    Ok(Snapshot {
        accounts,
        overview,
        overview_error,
    })
}

pub(crate) fn profile_directory() -> io::Result<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("LOCALAPPDATA");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    let home =
        home.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing profile directory"))?;
    Ok(PathBuf::from(home).join("Bytewarden.GPUI"))
}

fn device_id(root: &Path) -> io::Result<Uuid> {
    use std::io::Write;
    let path = root.join("device-id");
    match fs::read_to_string(&path) {
        Ok(text) => {
            Uuid::parse_str(text.trim()).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let id = Uuid::new_v4();
            // Persistent UUID is not a credential. Do not replace one created by another process.
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            writeln!(file, "{id}")?;
            file.sync_all()?;
            Ok(id)
        }
        Err(e) => Err(e),
    }
}

enum LocalUnlock {
    Password(Zeroizing<String>),
    Pin {
        account: String,
        pin: Zeroizing<String>,
    },
    Hello {
        account: String,
        window: usize,
    },
}

fn hello_hwnd(window: &Window) -> Option<usize> {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as usize),
            _ => None,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

pub(crate) fn status_text(err: &Error) -> String {
    match err.code() {
        "cancelled" => "操作已取消。",
        "timedOut" => "连接超时，请重试。",
        "locked" | "unlockUnavailable" => "保险库未解锁或本机解锁不可用。",
        "passwordRequired" | "wrongPassword" => "请验证主密码后重试。",
        "helloCancelled" => "Windows Hello 验证已取消，可使用主密码继续。",
        "helloUnavailable" => "Windows Hello 当前不可用，请使用主密码。",
        "helloFailed" => "Windows Hello 验证失败。请用主密码解锁后重新设置。",
        "helloNotEnabled" => "当前账户尚未启用 Windows Hello。",
        "wrongPin" => "PIN 码不正确，请重试（连续输错 5 次将退回主密码）。",
        "pinRevoked" => "PIN 码连续输错次数超限，已自动撤销并退回主密码解锁。",
        "pinNotEnabled" => "当前账户尚未启用 PIN 码解锁。",
        "localStorage" => "无法保存本机设置，请检查存储后重试。",
        "signInRequired" => "离线快照只读，请重新登录并同步。",
        "notSynced" => "本机存在尚待核对的写入结果。请同步后再继续操作。",
        "notFound" => "没有找到数据，或账户尚未同步。",
        "validation" => "请检查输入内容。",
        "cacheRevocationFailed" => "无法撤销本机缓存，请重试；若仍失败，请检查本机存储。",
        _ => "操作未完成。请重试或同步后核对。",
    }
    .into()
}

impl VaultPreview {
    /// Serialize UI operations. The core request ID is reserved before the
    /// worker starts, so cancellation also covers a job waiting in the queue.
    pub(crate) fn run<T: Send + 'static>(
        &mut self,
        label: &'static str,
        request: Option<Request>,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> T + Send + 'static,
        finish: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        assert!(self.client.busy.is_none());
        self.client.generation = self.client.generation.wrapping_add(1);
        let generation = self.client.generation;
        self.client.busy = Some(label);
        self.client.pending = request;
        let handle = cx.background_executor().spawn(async move { work() });
        cx.spawn(async move |weak, app| {
            let result = handle.await;
            let _ = weak.update(app, |this, cx| {
                if this.client.generation != generation {
                    return;
                }
                this.client.pending = None;
                this.client.busy = None;
                if this.closing {
                    // Drop any decrypted result without repopulating the closing UI.
                    return;
                }
                finish(this, result, cx);
                if std::mem::take(&mut this.lock_queued) {
                    this.request_lock(cx);
                }
                this.update_vault_timeouts(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn initialize(&mut self, cx: &mut Context<Self>) {
        if self.closing || self.client.initialized || self.client.busy.is_some() {
            return;
        }
        #[cfg(all(test, feature = "test-support"))]
        let fixture = self.client.initialization_results.pop_front();
        self.run(
            "正在读取本机账户",
            None,
            cx,
            move || -> Result<_, String> {
                #[cfg(all(test, feature = "test-support"))]
                if let Some(result) = fixture {
                    return result;
                }
                let root = profile_directory().map_err(|_| "找不到 Windows 用户数据目录。")?;
                fs::create_dir_all(&root).map_err(|_| "无法创建 GPUI 配置目录。")?;
                desktop::configure_cache(&root)
                    .map_err(|_| "无法打开 GPUI 保险库缓存，可能已有另一个 GPUI 实例正在运行。")?;
                let device = device_id(&root).map_err(|_| "无法读取设备标识。")?;
                Ok((device, snapshot().map_err(|e| status_text(&e))?))
            },
            move |this, result, cx| match result {
                Ok((device, snapshot)) => {
                    this.client.initialized = true;
                    this.client.device = Some(device);
                    this.apply_snapshot(snapshot, cx);
                    this.session_notice();
                    #[cfg(not(test))]
                    if !this.started_hidden && this.client.state() == Session::Locked {
                        let handle = this.window_handle;
                        let weak = cx.weak_entity();
                        cx.defer(move |cx| {
                            let _ = handle.update(cx, |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.client.form.is_none() {
                                        this.open_unlock(window, cx);
                                    }
                                });
                            });
                        });
                    }
                }
                Err(message) => this.client.notice = message,
            },
        );
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        let Snapshot {
            accounts,
            overview,
            overview_error,
        } = snapshot;
        self.client.accounts = accounts;
        self.client.overview_error = overview_error.as_ref().map(status_text);
        self.client.write_guard = overview_error.is_some();
        if let Some(error) = &self.client.overview_error {
            self.client.notice = error.clone();
        }
        if let Some(overview) = overview {
            self.client.set_overview(overview);
        } else {
            self.client.clear_overview();
        }
        let scope = crate::generator_preferences::Scope::for_account(self.client.active());
        self.generator.refresh_preference_scope(scope, cx);
        let icon_account = self
            .client
            .overview
            .as_ref()
            .and_then(|_| self.client.active());
        self.favicon_cache.set_scope(icon_account.map(|account| {
            (
                account.id.as_str(),
                account.server_mode,
                account.server_url.as_str(),
            )
        }));
        self.invalidate_query();
        self.broadcast_browser_state();
    }

    pub(crate) fn mark_write_uncertain(&mut self, error: &Error) {
        self.client.notice = status_text(error);
        if matches!(error.code(), "conflict" | "notSynced" | "writeUncertain") {
            self.client.write_guard = true;
            self.client.overview_error = Some(status_text(error));
        }
    }

    pub(crate) fn session_notice(&mut self) {
        if self.client.state() == Session::Unlocked
            && let Some(issue) = &self.client.overview_error
        {
            self.client.notice = issue.clone();
            return;
        }
        self.client.notice = match self.client.state() {
            Session::Locked => "保险库已锁定。可离线解锁本机加密快照。",
            Session::SignedOut => "请先登录。登录后同步并在本机解密保险库。",
            Session::Unlocked
                if self
                    .client
                    .overview
                    .as_ref()
                    .is_some_and(|overview| overview.read_only) =>
            {
                "本机离线快照 · 只读。"
            }
            Session::Unlocked if self.client.overview.is_some() => "保险库已加载。",
            Session::Unlocked => "已登录，请同步保险库。",
        }
        .into();
    }

    pub fn cancel_pending(&mut self, cx: &mut Context<Self>) {
        if let Some(req) = self.client.pending.take() {
            req.cancel();
            if self.client.form == Some(Form::SignIn) && !self.client.local_unlock_pending {
                self.restart_login(cx);
            }
            self.client.notice = "正在取消操作…".into();
            cx.notify();
        }
    }

    pub fn close_detail(&mut self) {
        self.selected = None;
        self.selected_item_id = None;
        self.confirm_attachment = None;
        self.pending_quick_copy = None;
        self.ui.detail_loading_since = None;
        self.client.redact_detail();
        self.detail_motion = None;
    }

    pub fn clear_account_view(&mut self, cx: &mut Context<Self>) {
        self.favicon_cache.clear();
        self.close_detail();
        self.clear_clipboard(cx);
        self.flush_generator_preferences(cx);
        self.generator.clear_inputs(cx);
        self.generator.clear();
        self.folders = crate::folders::Folders::default();
        self.sends.clear(cx);
        self.backup.clear(cx);
        self.health.clear();
        self.client.clear_overview();
        self.search_index = crate::vault_list::Index::default();
        self.index_generation = None;
        self.client.search.update(cx, |field, cx| field.set("", cx));
        self.client.selected_folder = None;
        self.client.show_folders = false;
        self.client.show_accounts = false;
        self.close_form(cx);
        self.invalidate_query();
        self.broadcast_browser_state();
    }

    pub(crate) fn close_form(&mut self, cx: &mut Context<Self>) {
        if self.client.local_unlock_pending {
            // A local unlock may commit before its worker reply reaches the UI.
            // Invalidate the request now and always reconcile via a compensating lock.
            self.lock_queued = true;
            self.cancel_pending(cx);
        }
        if matches!(
            self.client.form,
            Some(Form::SignIn | Form::HelloEnrollment | Form::PinEnrollment)
        ) && let Some(request) = self.client.pending.take()
        {
            request.cancel();
        }
        if self.client.form == Some(Form::SignIn) {
            desktop::cancel_login();
        }
        self.client.form_generation = self.client.form_generation.wrapping_add(1);
        self.client.form = None;
        self.client.reconnect_account = None;
        self.client.hello_window = None;
        if let Some(editor) = &mut self.client.editor {
            editor.clear(cx);
        }
        self.client.editor = None;
        self.clone_pending = false;
        self.client.login_challenge.reset();
        self.client.remember = false;
        self.client
            .password
            .update(cx, |field, cx| field.set("", cx));
        self.client.pin.update(cx, |field, cx| field.set("", cx));
        self.client.code.update(cx, |field, cx| field.set("", cx));
    }

    pub fn begin_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.client.ready() {
            return;
        }
        self.close_form(cx);
        self.close_detail();
        self.client.show_accounts = false;
        self.client.hello_window = hello_hwnd(window);
        self.client.form = Some(Form::SignIn);
        self.client.email.read(cx).focus(window);
        cx.notify();
    }

    pub(crate) fn reauthenticate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.client.ready() {
            return;
        }
        let Some(account) = self.client.active() else {
            return;
        };
        let id = account.id.clone();
        self.begin_sign_in(window, cx);
        self.client.reconnect_account = Some(id);
        self.client.password.read(cx).focus(window);
    }

    pub fn select_account(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.client.ready() {
            return;
        }
        self.clear_account_view(cx);
        self.run(
            "正在切换账户",
            None,
            cx,
            move || desktop::switch_account(&id).map(|_| ()).and(snapshot()),
            |this, result, cx| match result {
                Ok(state) => {
                    this.apply_snapshot(state, cx);
                    this.session_notice();
                }
                Err(e) => this.client.notice = status_text(&e),
            },
        );
    }

    pub fn request_lock(&mut self, cx: &mut Context<Self>) {
        let state = self.client.state();
        let auth_in_flight = self.client.session_transition_pending;
        if state != Session::Unlocked && !auth_in_flight && desktop::session() != Session::Unlocked
        {
            return;
        }
        // Queue locking across any in-flight operation that can change session
        // state, including an unlock or first sign-in. The completion path will
        // reconcile its snapshot before reapplying the lock intent.
        if self.client.busy.is_some() {
            self.lock_queued = true;
            self.clear_account_view(cx);
            self.cancel_pending(cx);
            return;
        }
        self.clear_account_view(cx);
        self.run(
            "正在锁定",
            None,
            cx,
            move || {
                let status = desktop::lock();
                (status, snapshot())
            },
            |this, (status, state), cx| {
                if let Ok(state) = state {
                    this.apply_snapshot(state, cx);
                }
                if let Err(error) = status {
                    this.client.notice = status_text(&error);
                } else {
                    this.session_notice();
                }
            },
        );
    }

    pub fn request_logout(&mut self, cx: &mut Context<Self>) {
        if !self.client.ready() || self.client.active().is_none() {
            return;
        }
        let preference_scope = self
            .client
            .active()
            .and_then(|account| account.settings_scope().ok());
        self.clear_account_view(cx);
        self.run(
            "正在退出登录",
            None,
            cx,
            || (desktop::logout(), snapshot()),
            move |this, (result, state), cx| {
                if let Ok(state) = state {
                    this.apply_snapshot(state, cx);
                }
                if result.is_ok()
                    && let Some(scope) = preference_scope.as_deref()
                {
                    this.generator.reset_saved_selection(scope);
                }
                if let Err(err) = result {
                    this.client.notice = status_text(&err);
                } else {
                    this.session_notice();
                }
            },
        );
    }

    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.client.ready()
            || !self.client.password.read(cx).committed()
            || !self.client.pin.read(cx).committed()
            || !self.client.code.read(cx).committed()
        {
            return;
        }
        match self.client.form {
            Some(Form::SignIn) => self.request_login(cx),
            Some(Form::Unlock) => {
                let use_pin = self.client.unlock_with_pin
                    && self
                        .client
                        .local_unlock_account()
                        .is_some_and(|a| a.pin_enabled);
                if use_pin {
                    self.request_pin_unlock(cx);
                } else {
                    self.request_unlock(cx);
                }
            }
            Some(Form::HelloEnrollment) => self.enroll_hello(cx),
            Some(Form::PinEnrollment) => self.enroll_pin(cx),
            Some(Form::EditorPassword) => {
                let secret = self.client.password.update(cx, |field, cx| field.take(cx));
                if !secret.is_empty() {
                    self.fetch_editor(secret, cx);
                }
            }
            Some(Form::Detail) => {
                if let Some(index) = self.selected {
                    let secret = self.client.password.update(cx, |field, cx| field.take(cx));
                    if !secret.is_empty() {
                        self.fetch_detail(index, secret, cx);
                    }
                }
            }
            Some(Form::Confirm(action) | Form::Reverify(action)) => {
                let secret = self.client.password.update(cx, |field, cx| field.take(cx));
                self.mutate_selected(action, secret, cx);
            }
            None => {}
        }
    }

    fn run_login(&mut self, mode: LoginMode, cx: &mut Context<Self>) {
        let Some(device) = self.client.device else {
            return;
        };
        let (email, url, region) = match self.client.reconnect() {
            Some(account) => (
                account.email.clone(),
                account.server_url.clone(),
                Region::from_mode(account.server_mode),
            ),
            None if self.client.reconnect_account.is_none() => (
                self.client.email.read(cx).value().trim().to_owned(),
                self.client.server.read(cx).value().trim().to_owned(),
                self.client.region,
            ),
            None => return,
        };
        if email.is_empty()
            || self.client.password.read(cx).value().is_empty()
            || !region.server(&url).is_valid()
        {
            self.client.notice = "请检查电子邮箱、主密码和服务器地址。".into();
            cx.notify();
            return;
        }
        let challenge = self.client.login_challenge.challenge;
        if let Some(Challenge::TwoFactor(options)) = challenge
            && !options.offers(self.client.provider)
        {
            self.client.notice =
                "账户要求的二步验证方式暂不支持，请使用网页保险库或官方客户端。".into();
            cx.notify();
            return;
        }
        if matches!(mode, LoginMode::Login)
            && challenge.is_some()
            && self.client.code.read(cx).value().is_empty()
        {
            self.client.notice = "请输入验证码。".into();
            cx.notify();
            return;
        }
        let password = Zeroizing::new(self.client.password.read(cx).value().to_string());
        let mut code = self.client.code.update(cx, |field, cx| field.take(cx));
        let device_code = if matches!(mode, LoginMode::Login) {
            self.client.login_challenge.device_code(&mut code)
        } else {
            Zeroizing::default()
        };
        let login_generation = self.client.login_challenge.generation;
        let provider = self.client.provider;
        let remember = self.client.remember;
        let enroll_hello_after_login = cfg!(windows) && self.client.enroll_hello_on_login;
        let hello_hwnd_for_login = self.client.hello_window;
        if matches!(mode, LoginMode::Login) {
            self.client.session_transition_pending = true;
        }
        let req = Request::begin();
        self.client.freeze_auth_inputs(true, cx);
        self.run(
            match mode {
                    LoginMode::Login => "正在登录并同步",
                _ => "正在发送验证码",
            },
            Some(req),
            cx,
            move || {
                let result = match mode {
                    LoginMode::Login => req.login(Login {
                        email: &email,
                        password: &password,
                        server: region.server(&url),
                        device,
                        two_factor: matches!(challenge, Some(Challenge::TwoFactor(_)))
                            .then_some((provider, code.as_str())),
                        device_code: &device_code,
                        remember_2fa: remember,
                    }),
                    LoginMode::Email => req.send_email_code(&email, &password, region.server(&url)),
                    LoginMode::Device => {
                        req.resend_device_code(&email, &password, device, region.server(&url))
                    }
                };
                let state = matches!(
                    (mode, &result),
                    (LoginMode::Login, Ok(AuthStatus::Authenticated))
                )
                .then(|| match req.sync() {
                    Ok(overview) => {
                        let mut hello_enrolled = false;
                        if enroll_hello_after_login
                            && let Some(hwnd) = hello_hwnd_for_login
                            && let Ok(accts) = desktop::accounts()
                            && let Some(active) = accts.iter().find(|a| a.active)
                            && !active.hello_enabled
                        {
                            hello_enrolled = req.enable_hello(&active.id, &password, hwnd).is_ok();
                        }
                        (
                            desktop::accounts().map(|accounts| Snapshot {
                                accounts,
                                overview: Some(overview),
                                overview_error: None,
                            }),
                            None,
                            hello_enrolled,
                        )
                    }
                    Err(error) => (snapshot(), Some(error), false),
                });
                drop(password);
                (result, state, device_code)
            },
            move |this, (result, state, device_code), cx| {
                this.client.freeze_auth_inputs(false, cx);
                if matches!(mode, LoginMode::Login) {
                    this.client.session_transition_pending = false;
                }
                // A dismissed login form cannot undo authentication already
                // committed by the core. Reconcile that session and immediately
                // lock it rather than leaving an invisible unlocked account.
                if !this.client.login_challenge.is_current(login_generation) {
                    if matches!(mode, LoginMode::Login)
                        && matches!(&result, Ok(AuthStatus::Authenticated))
                    {
                        this.clear_account_view(cx);
                        this.lock_queued = true;
                        if let Some((Ok(state), _, _)) = state {
                            this.apply_snapshot(state, cx);
                            this.client.notice =
                                "登录已完成，但登录窗口已关闭；正在锁定保险库。".into();
                        }
                    }
                    return;
                }
                match result {
                    Ok(AuthStatus::Authenticated) if matches!(mode, LoginMode::Login) => {
                        this.clear_account_view(cx);
                        this.ui.trigger_page_motion(this.animate);
                        let (state, sync_error, hello_enrolled) =
                            state.expect("successful login has account snapshot");
                        if hello_enrolled {
                            this.settings.auto_prompt_hello = true;
                            this.settings.save_silent();
                        }
                        match state {
                            Ok(state) => {
                                this.apply_snapshot(state, cx);
                                this.session_notice();
                                if let Some(error) = sync_error {
                                    this.client.notice =
                                        format!("已登录，但同步未完成。{}", status_text(&error));
                                } else if hello_enrolled {
                                    this.client.notice =
                                        "已登录并开启 Windows Hello 面部/指纹免密解锁。".into();
                                    this.ui.toast.show(
                                        crate::ui::toast::ToastKind::Success,
                                        "已开启 Windows Hello 免密刷脸解锁",
                                        std::time::Instant::now(),
                                        this.animate,
                                    );
                                }
                            }
                            Err(e) => this.client.notice = status_text(&e),
                        }
                    }
                    Ok(AuthStatus::Authenticated) => {
                        this.client.notice = "已请求验证码，请查看邮件。".into();
                    }
                    Ok(AuthStatus::TwoFactor(providers)) => {
                        let options = TwoFactorOptions::from_providers(region, providers);
                        let was_challenge = matches!(challenge, Some(Challenge::TwoFactor(_)));
                        // Only a login response may carry an accepted device OTP
                        // into the next challenge; email requests never replace it.
                        if matches!(mode, LoginMode::Login) {
                            this.client.login_challenge.two_factor(options, device_code);
                        } else {
                            this.client.login_challenge.challenge =
                                Some(Challenge::TwoFactor(options));
                        }
                        if let Some(provider) = options.selected(this.client.provider) {
                            this.client.provider = provider;
                        }
                        if was_challenge {
                            this.ui.trigger_auth_shake(this.animate);
                        } else {
                            this.ui.trigger_auth_step(this.animate);
                        }
                        this.client.notice = if options.manual_authenticator && was_challenge {
                            "服务器没有接受此次验证。如果确定同一账户启用了身份验证器，可用下一组 6 位验证码重试；否则请用网页保险库或官方客户端。"
                        } else if options.manual_authenticator {
                            "服务器没有列出此应用支持的二步验证方式。如果你确实为此账户启用了 TOTP，可手动输入 6 位验证码；是否接受仍由服务器决定。"
                        } else if options.selected(this.client.provider).is_some() {
                            "需要二步验证。"
                        } else {
                            "账户要求的二步验证方式暂不支持，请使用网页保险库或官方客户端。"
                        }
                        .into();
                    }
                    Ok(AuthStatus::DeviceVerification) => {
                        this.client.login_challenge.device();
                        this.ui.trigger_auth_step(this.animate);
                        this.client.notice = "需要验证新设备，输入邮件中的验证码。".into();
                    }
                    Ok(status) => {
                        this.client.notice = auth_message(status).into();
                        this.ui.trigger_auth_shake(this.animate);
                        if this
                            .client
                            .login_challenge
                            .failure(Some(status), !device_code.is_empty())
                        {
                            this.restart_login(cx);
                        }
                    }
                    Err(e) => {
                        this.client.notice = status_text(&e);
                        this.ui.trigger_auth_shake(this.animate);
                        this.client
                            .login_challenge
                            .failure(None, !device_code.is_empty());
                        this.restart_login(cx);
                    }
                }
            },
        );
    }

    fn restart_login(&mut self, cx: &mut Context<Self>) {
        let account = self.client.reconnect_account.take();
        let hw = self.client.hello_window;
        self.close_form(cx);
        self.client.form = Some(Form::SignIn);
        self.client.reconnect_account = account;
        self.client.hello_window = hw;
    }

    fn request_login(&mut self, cx: &mut Context<Self>) {
        self.run_login(LoginMode::Login, cx);
    }

    fn request_unlock(&mut self, cx: &mut Context<Self>) {
        if !self.client.ready()
            || self.client.local_unlock_account().is_none()
            || !self.client.password.read(cx).committed()
        {
            return;
        }
        let password = self.client.password.update(cx, |field, cx| field.take(cx));
        if password.is_empty() {
            return;
        }
        self.run_local_unlock(LocalUnlock::Password(password), cx);
    }

    fn request_pin_unlock(&mut self, cx: &mut Context<Self>) {
        if !self.client.ready() || !self.client.pin.read(cx).committed() {
            return;
        }
        let Some(account) = self
            .client
            .local_unlock_account()
            .filter(|account| account.pin_enabled)
        else {
            return;
        };
        let account = account.id.clone();
        let pin = self.client.pin.update(cx, |field, cx| field.take(cx));
        if pin.is_empty() {
            return;
        }
        self.run_local_unlock(LocalUnlock::Pin { account, pin }, cx);
    }

    pub(crate) fn change_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing || !self.client.ready() || !self.navigation_available() {
            return;
        }
        let Some(account) = self
            .client
            .active()
            .filter(|account| account.session() == Session::Unlocked)
        else {
            return;
        };
        if account.pin_enabled {
            let id = account.id.clone();
            self.run(
                "正在关闭 PIN 码解锁",
                None,
                cx,
                move || desktop::disable_pin(&id).and_then(|()| desktop::accounts()),
                |this, result, _| match result {
                    Ok(accounts) => {
                        this.client.accounts = accounts;
                        this.client.notice = "已关闭本机 PIN 码快速解锁。".into();
                        this.broadcast_browser_state();
                    }
                    Err(error) => this.client.notice = status_text(&error),
                },
            );
            return;
        }
        if self.client.overview.is_none() || self.client.write_guard {
            self.client.notice = "请先同步保险库。".into();
            cx.notify();
            return;
        }
        self.close_detail();
        self.close_form(cx);
        self.client.pin_require_mp_on_restart = false;
        self.client.form = Some(Form::PinEnrollment);
        self.client.password.read(cx).focus(window);
        cx.notify();
    }

    fn enroll_pin(&mut self, cx: &mut Context<Self>) {
        let Some(account) = self
            .client
            .active()
            .filter(|account| account.session() == Session::Unlocked && !account.pin_enabled)
        else {
            return;
        };
        let id = account.id.clone();
        let pin = Zeroizing::new(self.client.pin.read(cx).value().trim().to_owned());
        if pin.chars().count() < 4 {
            self.ui.trigger_auth_shake(self.animate);
            self.client.notice = "PIN 码至少需要 4 个字符。".into();
            cx.notify();
            return;
        }
        let password = self.client.password.update(cx, |input, cx| input.take(cx));
        if password.is_empty() {
            self.ui.trigger_auth_shake(self.animate);
            self.client.notice = "请输入主密码以确认身份。".into();
            cx.notify();
            return;
        }
        self.client.pin.update(cx, |input, cx| input.set("", cx));
        let require_mp_on_restart = self.client.pin_require_mp_on_restart;
        let request = Request::begin();
        let generation = self.client.form_generation;
        self.client.freeze_auth_inputs(true, cx);
        self.run(
            "正在加密保存 PIN 码",
            Some(request),
            cx,
            move || {
                let result = request.enable_pin(&id, &password, &pin, require_mp_on_restart);
                (result, desktop::accounts())
            },
            move |this, (result, accounts), cx| {
                this.client.freeze_auth_inputs(false, cx);
                if let Ok(accounts) = accounts {
                    this.client.accounts = accounts;
                }
                if this.client.form_generation != generation {
                    return;
                }
                match result {
                    Ok(()) => {
                        this.close_form(cx);
                        this.client.notice = "已启用本机 PIN 码快速解锁。".into();
                        this.broadcast_browser_state();
                        this.ui.toast.show(
                            crate::ui::toast::ToastKind::Success,
                            "已启用 PIN 码快速解锁",
                            std::time::Instant::now(),
                            this.animate,
                        );
                    }
                    Err(error) => {
                        this.ui.trigger_auth_shake(this.animate);
                        this.client.notice = status_text(&error);
                    }
                }
            },
        );
    }

    fn request_hello_unlock(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.client.ready() {
            return;
        }
        let Some(account) = self
            .client
            .local_unlock_account()
            .filter(|account| account.hello_enabled)
        else {
            return;
        };
        let Some(window) = hello_hwnd(window) else {
            self.client.notice = "Windows Hello 当前不可用，请使用主密码。".into();
            cx.notify();
            return;
        };
        let account = account.id.clone();
        self.client
            .password
            .update(cx, |field, cx| field.set("", cx));
        self.run_local_unlock(LocalUnlock::Hello { account, window }, cx);
    }

    pub(crate) fn change_hello(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing || !self.client.ready() || !self.navigation_available() {
            return;
        }
        let Some(account) = self
            .client
            .active()
            .filter(|account| account.session() == Session::Unlocked)
        else {
            return;
        };
        if account.hello_enabled {
            let id = account.id.clone();
            self.run(
                "正在关闭 Windows Hello",
                None,
                cx,
                move || desktop::disable_hello(&id).and_then(|()| desktop::accounts()),
                |this, result, _| match result {
                    Ok(accounts) => {
                        this.client.accounts = accounts;
                        this.client.notice = "已关闭 Windows Hello 解锁。".into();
                    }
                    Err(error) => this.client.notice = status_text(&error),
                },
            );
            return;
        }
        if self.client.overview.is_none() || self.client.write_guard {
            self.client.notice = "请先同步保险库。".into();
            cx.notify();
            return;
        }
        let Some(handle) = hello_hwnd(window) else {
            self.client.notice = "Windows Hello 当前不可用。".into();
            cx.notify();
            return;
        };
        self.close_detail();
        self.close_form(cx);
        self.client.hello_window = Some(handle);
        self.client.form = Some(Form::HelloEnrollment);
        self.client.password.read(cx).focus(window);
        cx.notify();
    }

    fn enroll_hello(&mut self, cx: &mut Context<Self>) {
        let Some(window) = self.client.hello_window else {
            return;
        };
        let Some(account) = self
            .client
            .active()
            .filter(|account| account.session() == Session::Unlocked && !account.hello_enabled)
        else {
            return;
        };
        let id = account.id.clone();
        let password = self.client.password.update(cx, |input, cx| input.take(cx));
        if password.is_empty() {
            return;
        }
        let request = Request::begin();
        let generation = self.client.form_generation;
        self.client.freeze_auth_inputs(true, cx);
        self.run(
            "请完成 Windows Hello 设置",
            Some(request),
            cx,
            move || {
                let result = request.enable_hello(&id, &password, window);
                (result, desktop::accounts())
            },
            move |this, (result, accounts), cx| {
                this.client.freeze_auth_inputs(false, cx);
                if let Ok(accounts) = accounts {
                    this.client.accounts = accounts;
                }
                if this.client.form_generation != generation {
                    return;
                }
                match result {
                    Ok(()) => {
                        this.settings.auto_prompt_hello = true;
                        this.settings.save_silent();
                        this.close_form(cx);
                        this.client.notice = "已启用 Windows Hello 面部/指纹免密解锁。".into();
                        this.broadcast_browser_state();
                        this.ui.toast.show(
                            crate::ui::toast::ToastKind::Success,
                            "已启用 Windows Hello 免密刷脸解锁",
                            std::time::Instant::now(),
                            this.animate,
                        );
                    }
                    Err(error) => {
                        this.ui.trigger_auth_shake(this.animate);
                        this.client.notice = status_text(&error);
                    }
                }
            },
        );
    }

    fn run_local_unlock(&mut self, method: LocalUnlock, cx: &mut Context<Self>) {
        let req = Request::begin();
        let generation = self.client.form_generation;
        self.client.session_transition_pending = true;
        self.client.local_unlock_pending = true;
        self.client.freeze_auth_inputs(true, cx);
        self.run(
            if matches!(&method, LocalUnlock::Hello { .. }) {
                "请完成 Windows Hello 验证"
            } else if matches!(&method, LocalUnlock::Pin { .. }) {
                "正在验证 PIN 码"
            } else {
                "正在离线解锁"
            },
            Some(req),
            cx,
            move || {
                let res = match method {
                    LocalUnlock::Password(password) => req.unlock(&password),
                    LocalUnlock::Pin { account, pin } => req.unlock_pin(&account, &pin),
                    LocalUnlock::Hello { account, window } => req.unlock_hello(&account, window),
                };
                match res {
                    Ok(()) => snapshot().map_err(|e| (e, None)),
                    Err(err) => {
                        let refreshed = (err.code() == "pinRevoked")
                            .then(|| desktop::accounts().ok())
                            .flatten();
                        Err((err, refreshed))
                    }
                }
            },
            move |this, result, cx| {
                this.client.freeze_auth_inputs(false, cx);
                this.client.session_transition_pending = false;
                this.client.local_unlock_pending = false;
                if this.client.form_generation != generation || this.closing {
                    // Never install decrypted UI state after the user dismissed
                    // the unlock form. If the core already committed, `run` will
                    // execute the queued compensating lock immediately afterward.
                    this.lock_queued = true;
                    if let Err((error, _)) = result {
                        this.client.notice = status_text(&error);
                    }
                    return;
                }
                match result {
                    Ok(state) => {
                        let should_auto_sync =
                            state.overview.as_ref().is_some_and(|ov| ov.read_only)
                                && state
                                    .accounts
                                    .iter()
                                    .find(|a| a.active)
                                    .is_some_and(|a| !a.needs_sign_in);
                        this.close_form(cx);
                        this.apply_snapshot(state, cx);
                        this.ui.trigger_page_motion(this.animate);
                        this.session_notice();
                        if should_auto_sync && !this.lock_queued {
                            this.request_sync(cx);
                        }
                    }
                    Err((e, refreshed_accounts)) => {
                        if let Some(accounts) = refreshed_accounts {
                            this.client.accounts = accounts;
                            this.client.unlock_with_pin = false;
                            this.broadcast_browser_state();
                        }
                        this.ui.trigger_auth_shake(this.animate);
                        this.client.notice = status_text(&e);
                    }
                }
            },
        );
    }

    pub(crate) fn accept_synced_overview(&mut self, overview: Overview) {
        self.client.set_overview(overview);
        self.client.overview_error = None;
        self.client.write_guard = false;
        self.invalidate_query();
        self.session_notice();
        self.broadcast_browser_state();
    }

    pub fn request_sync(&mut self, cx: &mut Context<Self>) {
        if !self.client.sync_available()
            || self.client.form.is_some()
            || self.client.editor.is_some()
        {
            return;
        }
        self.close_detail();
        let req = Request::begin();
        self.run(
            "正在同步",
            Some(req),
            cx,
            move || match req.sync() {
                Ok(overview) => Ok(overview),
                Err(e) => {
                    let refreshed = (e.code() == "signInRequired")
                        .then(|| desktop::accounts().ok())
                        .flatten();
                    Err((e, refreshed))
                }
            },
            |this, result, _| match result {
                Ok(overview) => this.accept_synced_overview(overview),
                Err((e, refreshed)) => {
                    if let Some(accounts) = refreshed {
                        this.client.accounts = accounts;
                        this.broadcast_browser_state();
                    }
                    this.client.notice = status_text(&e);
                }
            },
        );
    }

    pub(crate) fn sync_or_reconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.client.reconnect_available() && self.navigation_available() {
            self.reauthenticate(window, cx);
        } else if self.selection.pending.is_none() {
            self.request_sync(cx);
        }
    }

    pub fn choose_item(&mut self, index: usize, cx: &mut Context<Self>) {
        self.pending_quick_copy = None;
        if !self.client.ready() || self.client.form.is_some() || self.client.editor.is_some() {
            return;
        }
        if self.selected == Some(index) && self.client.detail.is_some() {
            return;
        }
        let Some(item_id) = self
            .client
            .overview
            .as_ref()
            .and_then(|overview| overview.items.get(index))
            .map(|item| item.id.clone())
        else {
            return;
        };
        self.close_detail();
        self.selected = Some(index);
        self.selected_item_id = Some(item_id);
        self.ui.detail_loading_since = Some(std::time::Instant::now());
        self.ui.trigger_vault_narrow(true, self.animate);
        self.fetch_detail(index, Zeroizing::default(), cx);
    }

    pub(crate) fn request_quick_copy(
        &mut self,
        item_id: String,
        action: QuickCopyAction,
        cx: &mut Context<Self>,
    ) {
        if !self.client.ready()
            || self.client.busy.is_some()
            || self.client.form.is_some()
            || self.client.editor.is_some()
            || self.selection.active
            || self.lock_queued
        {
            return;
        }
        let Some(index) = self.client.overview.as_ref().and_then(|overview| {
            overview.items.iter().position(|item| {
                item.id == item_id && !item.deleted && !item.restricted && item.item_type == 1
            })
        }) else {
            return;
        };
        if self.selected == Some(index)
            && self.selected_item_id.as_deref() == Some(item_id.as_str())
            && self.client.detail.is_some()
        {
            self.pending_quick_copy = Some(PendingQuickCopy { item_id, action });
            self.complete_pending_quick_copy(cx);
            return;
        }
        self.close_detail();
        self.selected = Some(index);
        self.selected_item_id = Some(item_id.clone());
        self.pending_quick_copy = Some(PendingQuickCopy { item_id, action });
        self.fetch_detail(index, Zeroizing::default(), cx);
    }

    fn fetch_detail(&mut self, index: usize, password: Zeroizing<String>, cx: &mut Context<Self>) {
        let Some(id) = self
            .client
            .overview
            .as_ref()
            .and_then(|overview| overview.items.get(index))
            .map(|item| item.id.clone())
        else {
            return;
        };
        let request_id = id.clone();
        let req = Request::begin();
        self.run(
            "正在读取条目",
            Some(req),
            cx,
            move || req.detail(&request_id, &password),
            move |this, result, cx| match result {
                Ok(detail)
                    if this.selected == Some(index)
                        && this.selected_item_id.as_deref() == Some(id.as_str())
                        && !this.lock_queued =>
                {
                    this.close_form(cx);
                    this.ui.detail_loading_since = None;
                    this.client.detail = Some(detail);
                    this.watch_totp(cx);
                    this.complete_pending_quick_copy(cx);
                    this.detail_motion = this.transition();
                    this.session_notice();
                }
                Ok(detail) => detail.revoke(),
                Err(e)
                    if matches!(e.code(), "passwordRequired" | "wrongPassword")
                        && this.selected == Some(index)
                        && this.selected_item_id.as_deref() == Some(id.as_str()) =>
                {
                    this.ui.detail_loading_since = None;
                    if this.client.form == Some(Form::Detail) {
                        this.ui.trigger_auth_shake(this.animate);
                    }
                    this.client.form = Some(Form::Detail);
                    this.client.notice = status_text(&e);
                }
                Err(e) => {
                    this.close_detail();
                    this.close_form(cx);
                    this.client.notice = status_text(&e);
                }
            },
        );
    }

    pub fn begin_action(&mut self, action: ItemAction, cx: &mut Context<Self>) {
        if !self.client.ready() || self.client.form.is_some() {
            return;
        }
        if matches!(action, ItemAction::Delete) {
            self.client.form = Some(Form::Confirm(action));
            cx.notify();
        } else {
            self.mutate_selected(action, Zeroizing::default(), cx);
        }
    }

    fn mutate_selected(
        &mut self,
        action: ItemAction,
        password: Zeroizing<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self
            .client
            .overview
            .as_ref()
            .filter(|overview| !overview.read_only)
            .and_then(|overview| overview.items.get(self.selected?))
        else {
            return;
        };
        let (id, rev) = (item.id.clone(), item.revision_date.clone());
        self.client.redact_detail();
        let req = Request::begin();
        self.run(
            "正在保存",
            Some(req),
            cx,
            move || req.action(&id, action, &rev, &password),
            move |this, result, cx| match result {
                Ok(overview) => {
                    this.close_form(cx);
                    this.close_detail();
                    this.client.set_overview(overview);
                    this.invalidate_query();
                    this.session_notice();
                }
                Err(e) if matches!(e.code(), "passwordRequired" | "wrongPassword") => {
                    this.client.form = Some(Form::Reverify(action));
                    this.client.notice = status_text(&e);
                }
                Err(e) => {
                    this.close_form(cx);
                    this.close_detail();
                    this.client.notice = status_text(&e);
                }
            },
        );
    }

    pub fn open_unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.client.ready() || self.client.state() != Session::Locked {
            return;
        }
        if self
            .client
            .active()
            .is_some_and(|account| account.needs_sign_in)
        {
            self.reauthenticate(window, cx);
        } else {
            let prefer_pin = self
                .client
                .local_unlock_account()
                .is_some_and(|a| a.pin_enabled);
            self.close_form(cx);
            self.client.show_accounts = false;
            self.client.unlock_with_pin = prefer_pin;
            self.client.form = Some(Form::Unlock);
            if prefer_pin {
                self.client.pin.read(cx).focus(window);
            } else {
                self.client.password.read(cx).focus(window);
            }
        }
        if self.settings.auto_prompt_hello {
            self.request_hello_unlock(window, cx);
        }
        cx.notify();
    }

    pub fn form_modal_snapshot(&self) -> Option<crate::ui::state::ModalClosingSnapshot> {
        let form = self.client.form?;
        if matches!(form, Form::SignIn | Form::Unlock) {
            return None;
        }
        Some(match form {
            Form::Detail | Form::EditorPassword | Form::Reverify(_) => {
                crate::ui::state::ModalClosingSnapshot {
                    title: "验证主密码".into(),
                    subtitle: "此条目受主密码复验保护，请输入主密码继续。".into(),
                    danger: false,
                    action_label: "验证",
                }
            }
            Form::Confirm(_) => crate::ui::state::ModalClosingSnapshot {
                title: "永久删除此条目？".into(),
                subtitle: "该操作不可撤销，永久删除后无法恢复。".into(),
                danger: true,
                action_label: "确认永久删除",
            },
            Form::HelloEnrollment => crate::ui::state::ModalClosingSnapshot {
                title: "启用 Windows Hello 解锁".into(),
                subtitle: "先确认主密码，再完成 Windows Hello 验证。".into(),
                danger: false,
                action_label: "继续",
            },
            Form::PinEnrollment => crate::ui::state::ModalClosingSnapshot {
                title: "设置 PIN 码快速解锁".into(),
                subtitle: "使用 Argon2 与受保护密钥信封加密本地密钥，输错 5 次自动回退主密码。"
                    .into(),
                danger: false,
                action_label: "启用 PIN",
            },
            Form::SignIn | Form::Unlock => unreachable!(),
        })
    }

    pub fn render_form(&self, now: std::time::Instant, cx: &mut Context<Self>) -> gpui::Div {
        use crate::theme::{self, hex};
        use crate::ui::{
            brand::brand_logo,
            components::{
                BtnVariant, badge, btn, elevated_card, glyph_icon, pill_chip, segmented_track,
                toggle_switch_animated,
            },
            spring::clamp_opacity,
            tokens::{font, glyph, radius, space},
        };

        let pal = theme::palette();
        let form = self.client.form.expect("form visible");
        let reconnect = self.client.reconnect();
        let allow_local = self.client.local_unlock_account().is_some();
        let offer_hello = cfg!(windows)
            && self
                .client
                .local_unlock_account()
                .is_some_and(|account| account.hello_enabled);
        let ready = self.client.ready();
        let is_full_auth = matches!(form, Form::SignIn | Form::Unlock);
        let (shake_val, _) = self.ui.auth_shake_spring.sample_staggered(
            now,
            std::time::Duration::ZERO,
            self.animate,
        );
        let (step_p, _) =
            self.ui
                .auth_step_spring
                .sample_staggered(now, std::time::Duration::ZERO, self.animate);
        let shake_x = (shake_val * 2.6).clamp(-10., 10.);
        let title = match form {
            Form::SignIn if allow_local => "解锁保险库",
            Form::SignIn if reconnect.is_some() => "重新连接账户",
            Form::SignIn => "登录 Bytewarden",
            Form::Unlock => "解锁保险库",
            Form::HelloEnrollment => "启用 Windows Hello 解锁",
            Form::PinEnrollment => "设置 PIN 码快速解锁",
            Form::Detail | Form::EditorPassword | Form::Reverify(_) => "验证主密码",
            Form::Confirm(_) => "永久删除此条目？",
        };
        let offer_pin = self
            .client
            .local_unlock_account()
            .is_some_and(|account| account.pin_enabled);
        let using_pin_unlock = form == Form::Unlock && offer_pin && self.client.unlock_with_pin;
        let subtitle = match form {
            Form::SignIn if allow_local => "使用主密码连接服务器并同步，或仅离线解锁本机快照。",
            Form::SignIn if reconnect.is_some() => "登录后同步此账户的最新数据。",
            Form::SignIn => "连接到 Bitwarden 官方云或自托管服务器",
            Form::Unlock if using_pin_unlock => {
                "输入本地短 PIN 码快速解密保险库（连续输错 5 次自动回退主密码）"
            }
            Form::Unlock => "输入主密码以离线解密本机快照",
            Form::HelloEnrollment => "先确认主密码，再完成 Windows Hello 验证。",
            Form::PinEnrollment => {
                "验证主密码并设置至少 4 位本地 PIN 码（连续输错 5 次自动回退主密码）。"
            }
            Form::Detail | Form::EditorPassword | Form::Reverify(_) => {
                "此条目受主密码复验保护，请输入主密码继续。"
            }
            Form::Confirm(_) => "该操作不可撤销，永久删除后无法恢复。",
        };
        let challenge = self.client.login_challenge.challenge;
        let active_account = self.client.active();

        let mut panel = elevated_card()
            .relative()
            .left(px(shake_x))
            .flex()
            .flex_col()
            .gap(px(space::M))
            .p(px(if is_full_auth { space::XL } else { 20. }))
            .max_w(px(if is_full_auth { 440. } else { 460. }))
            .w_full();

        if is_full_auth {
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .pb(px(4.))
                    .child(brand_logo(58.))
                    .child(
                        div()
                            .text_size(px(font::SUBTITLE))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(hex(pal.text))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_center()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_secondary))
                            .child(subtitle),
                    ),
            );
            if let Some(acc) = reconnect.or(active_account.filter(|_| form == Form::Unlock)) {
                panel =
                    panel.child(
                        div()
                            .p(px(10.))
                            .rounded(px(radius::CONTROL))
                            .bg(hex(pal.layer))
                            .border_1()
                            .border_color(hex(pal.border_subtle))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(hex(pal.text))
                                    .child(acc.email.clone()),
                            )
                            .child(div().flex().items_center().child(Self::render_server_badge(
                                acc.server_mode,
                                &acc.server_url,
                            ))),
                    );
            }
        } else {
            let is_danger = matches!(form, Form::Confirm(_));
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .size(px(34.))
                            .rounded(px(radius::PILL))
                            .bg(pal.tint(if is_danger {
                                pal.danger
                            } else {
                                pal.accent_fill
                            }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(hex(if is_danger {
                                pal.danger
                            } else {
                                pal.accent_text
                            }))
                            .child(glyph_icon(
                                if is_danger {
                                    glyph::DELETE
                                } else {
                                    glyph::LOCK
                                },
                                15.,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(font::BODY_LG))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(hex(pal.text))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child(subtitle),
                            ),
                    ),
            );
        }

        if form == Form::SignIn {
            if challenge.is_none() && reconnect.is_none() {
                let region_index = match self.client.region {
                    Region::Us => 0,
                    Region::Eu => 1,
                    Region::Custom => 2,
                };
                let region_pos = self.ui.segmented_position(
                    &gpui::ElementId::Name("login-region".into()),
                    region_index,
                    now,
                    self.animate,
                );
                panel = panel.child(
                    segmented_track(3, region_pos).children(
                        [
                            (Region::Us, "美国 (US)"),
                            (Region::Eu, "欧盟 (EU)"),
                            (Region::Custom, "自托管"),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(i, (region, label))| {
                            let selected = self.client.region == region;
                            div()
                                .id(("region", i))
                                .relative()
                                .cursor_pointer()
                                .flex_1()
                                .h(px(30.))
                                .rounded(px(radius::CONTROL))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(font::CAPTION))
                                .font_weight(if selected {
                                    gpui::FontWeight::SEMIBOLD
                                } else {
                                    gpui::FontWeight::NORMAL
                                })
                                .text_color(hex(if selected {
                                    pal.accent_text
                                } else {
                                    pal.text_secondary
                                }))
                                .when(!selected, |el| {
                                    el.hover(move |s| s.text_color(hex(pal.text)))
                                })
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.client.ready() && this.client.region != region {
                                        let from = match this.client.region {
                                            Region::Us => 0,
                                            Region::Eu => 1,
                                            Region::Custom => 2,
                                        };
                                        this.client.region = region;
                                        this.ui.animate_segmented(
                                            gpui::ElementId::Name("login-region".into()),
                                            from,
                                            i,
                                            this.animate,
                                        );
                                        cx.notify();
                                    }
                                }))
                        }),
                    ),
                );
                if self.client.region == Region::Custom {
                    panel = panel.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child("服务器地址"),
                            )
                            .child(self.client.server.clone()),
                    );
                }
                panel = panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.text_secondary))
                                .child("电子邮箱"),
                        )
                        .child(self.client.email.clone()),
                );
            } else if challenge.is_some() {
                panel = panel.child(
                    div()
                        .relative()
                        .left(px(14. * (1. - step_p)))
                        .opacity(clamp_opacity(step_p))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(badge("步骤 2 / 2", pal.accent_text))
                        .child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.text_secondary))
                                .child(if matches!(challenge, Some(Challenge::Device)) {
                                    "请输入邮箱收到的新设备验证码"
                                } else {
                                    "请完成二步验证以继续登录"
                                }),
                        ),
                );
            }
            if let Some(Challenge::TwoFactor(options)) = challenge
                && options.offers(self.client.provider)
            {
                let remember_p = self.ui.toggle_progress(
                    &gpui::ElementId::Name("remember-2fa".into()),
                    self.client.remember,
                    now,
                    self.animate,
                );
                panel = panel
                    .child(
                        div().flex().flex_wrap().gap(px(6.)).children(
                            PROVIDERS
                                .into_iter()
                                .enumerate()
                                .filter(|(_, (provider, _))| options.offers(*provider))
                                .map(|(i, (provider, label))| {
                                    let label = if options.manual_authenticator {
                                        "身份验证器（手动输入）"
                                    } else {
                                        label
                                    };
                                    pill_chip(
                                        ("provider", i),
                                        label,
                                        None,
                                        self.client.provider == provider,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            if this.client.ready() {
                                                this.client.provider = provider;
                                                this.client
                                                    .code
                                                    .update(cx, |field, cx| field.set("", cx));
                                                this.client.code.read(cx).focus(window);
                                                cx.notify();
                                            }
                                        },
                                    ))
                                }),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .py(px(4.))
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child("记住二步验证（30 天）"),
                            )
                            .child(
                                toggle_switch_animated(
                                    "remember-2fa",
                                    self.client.remember,
                                    remember_p,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if this.client.ready() {
                                            this.client.remember = !this.client.remember;
                                            this.ui.animate_toggle(
                                                "remember-2fa",
                                                this.client.remember,
                                                this.animate,
                                            );
                                            cx.notify();
                                        }
                                    },
                                )),
                            ),
                    );
                if self.client.provider == Provider::Email {
                    panel = panel.child(
                        btn("send-2fa-email", BtnVariant::Secondary)
                            .h(px(30.))
                            .text_size(px(font::CAPTION))
                            .child("发送邮箱验证码")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.client.ready() {
                                    this.run_login(LoginMode::Email, cx);
                                }
                            })),
                    );
                }
            } else if matches!(challenge, Some(Challenge::Device)) {
                panel = panel.child(
                    btn("resend-device-email", BtnVariant::Secondary)
                        .h(px(30.))
                        .text_size(px(font::CAPTION))
                        .child("重发新设备验证码")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.client.ready() {
                                this.run_login(LoginMode::Device, cx);
                            }
                        })),
                );
            }
            if matches!(challenge, Some(Challenge::Device))
                || matches!(challenge, Some(Challenge::TwoFactor(options)) if options.offers(self.client.provider))
            {
                panel = panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.text_secondary))
                                .child("验证码"),
                        )
                        .child(self.client.code.clone()),
                );
            }
        }
        if ready && offer_hello {
            panel = panel.child(
                btn("hello-unlock", BtnVariant::Primary)
                    .w_full()
                    .h(px(38.))
                    .child(glyph_icon(glyph::PASSKEY, 15.))
                    .child("Windows Hello 面部 / 指纹解锁")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.request_hello_unlock(window, cx)),
                    ),
            );
        }
        if form == Form::Unlock && offer_pin {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        pill_chip("unlock-mode-pin", "PIN 码解锁", None, using_pin_unlock)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.client.ready() {
                                    this.client.unlock_with_pin = true;
                                    this.client.pin.read(cx).focus(window);
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        pill_chip("unlock-mode-mp", "主密码解锁", None, !using_pin_unlock)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.client.ready() {
                                    this.client.unlock_with_pin = false;
                                    this.client.password.read(cx).focus(window);
                                    cx.notify();
                                }
                            })),
                    ),
            );
        }
        if using_pin_unlock {
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_secondary))
                            .child("PIN 码"),
                    )
                    .child(self.client.pin.clone()),
            );
        } else if matches!(
            form,
            Form::Unlock
                | Form::HelloEnrollment
                | Form::PinEnrollment
                | Form::Detail
                | Form::EditorPassword
                | Form::Reverify(_)
        ) || (form == Form::SignIn && challenge.is_none())
        {
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_secondary))
                            .child("主密码"),
                    )
                    .child(self.client.password.clone()),
            );
        }
        if form == Form::SignIn
            && challenge.is_none()
            && cfg!(windows)
            && !reconnect.is_some_and(|a| a.hello_enabled)
        {
            let hello_login_p = self.ui.toggle_progress(
                &gpui::ElementId::Name("login-enroll-hello".into()),
                self.client.enroll_hello_on_login,
                now,
                self.animate,
            );
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .py(px(2.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(hex(pal.text))
                                    .child("启用 Windows Hello 面部 / 指纹免密登录"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(hex(pal.text_secondary))
                                    .child("登录成功后自动绑定面部/生物识别，下次打开直接刷脸解锁并同步"),
                            ),
                    )
                    .child(
                        toggle_switch_animated(
                            "login-enroll-hello",
                            self.client.enroll_hello_on_login,
                            hello_login_p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.client.ready() {
                                this.client.enroll_hello_on_login =
                                    !this.client.enroll_hello_on_login;
                                this.ui.animate_toggle(
                                    "login-enroll-hello",
                                    this.client.enroll_hello_on_login,
                                    this.animate,
                                );
                                cx.notify();
                            }
                        })),
                    ),
            );
        }
        if form == Form::PinEnrollment {
            let require_mp_p = self.ui.toggle_progress(
                &gpui::ElementId::Name("pin-require-mp".into()),
                self.client.pin_require_mp_on_restart,
                now,
                self.animate,
            );
            panel = panel
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.text_secondary))
                                .child("设置 PIN 码（至少 4 位数字或字母）"),
                        )
                        .child(self.client.pin.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .py(px(2.))
                        .child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.text_secondary))
                                .child("每次重启应用后首次仍需输入主密码"),
                        )
                        .child(
                            toggle_switch_animated(
                                "pin-require-mp",
                                self.client.pin_require_mp_on_restart,
                                require_mp_p,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.client.ready() {
                                    this.client.pin_require_mp_on_restart =
                                        !this.client.pin_require_mp_on_restart;
                                    this.ui.animate_toggle(
                                        "pin-require-mp",
                                        this.client.pin_require_mp_on_restart,
                                        this.animate,
                                    );
                                    cx.notify();
                                }
                            })),
                        ),
                );
        }
        let submit_variant = if matches!(form, Form::Confirm(_)) {
            BtnVariant::Danger
        } else {
            BtnVariant::Primary
        };
        if ready && form == Form::SignIn && allow_local {
            panel = panel.child(
                div().flex().flex_wrap().gap(px(8.)).child(
                    btn("offline-unlock", BtnVariant::Secondary)
                        .child("仅离线解锁")
                        .on_click(cx.listener(|this, _, _, cx| this.request_unlock(cx))),
                ),
            );
        }
        panel.child(
            div()
                .pt(px(4.))
                .flex()
                .items_center()
                .gap(px(8.))
                .when(ready, |bar| {
                    bar.child(
                        btn("submit-form", submit_variant)
                            .flex_1()
                            .h(px(36.))
                            .child(match form {
                                Form::SignIn if allow_local => "解锁并同步",
                                Form::SignIn => "登录并同步",
                                Form::Unlock => "解锁",
                                Form::HelloEnrollment => "继续",
                                Form::PinEnrollment => "启用 PIN",
                                Form::Detail | Form::EditorPassword | Form::Reverify(_) => "验证",
                                Form::Confirm(_) => "确认永久删除",
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    )
                })
                .when(
                    ready
                        || is_full_auth
                        || matches!(form, Form::HelloEnrollment | Form::PinEnrollment),
                    |bar| {
                        bar.child(
                            btn("cancel-form", BtnVariant::Secondary)
                                .h(px(36.))
                                .px(px(16.))
                                .child(if ready { "返回" } else { "取消" })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_form(cx);
                                    this.close_detail();
                                    this.focus.focus(window);
                                    this.session_notice();
                                    cx.notify();
                                })),
                        )
                    },
                ),
        )
    }
}

#[cfg(test)]
#[path = "auth_challenge_tests.rs"]
mod auth_challenge_tests;

#[cfg(all(test, feature = "test-support"))]
mod initialization_tests {
    use super::*;
    #[gpui::test]
    fn initialization_failure_has_a_working_retry_button(gpui: &mut gpui::TestAppContext) {
        let (view, cx) = crate::workflow_tests::window(gpui);
        view.update(cx, |view, cx| {
            view.client
                .initialization_results
                .push_back(Err("synthetic cache unavailable".into()));
            view.client.initialization_results.push_back(Ok((
                Uuid::nil(),
                Snapshot {
                    accounts: Vec::new(),
                    overview: None,
                    overview_error: None,
                },
            )));
            view.initialize(cx);
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(!view.client.initialized);
            assert!(view.client.busy.is_none());
            assert_eq!(view.client.notice, "synthetic cache unavailable");
        });
        let bounds = cx
            .debug_bounds("retry-initialization")
            .expect("visible retry button after failure");
        cx.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.client.ready());
            assert_eq!(view.client.state(), Session::SignedOut);
            assert!(view.client.initialization_results.is_empty());
        });
        assert!(cx.debug_bounds("retry-initialization").is_none());
    }
}

#[derive(Clone, Copy)]
enum LoginMode {
    Login,
    Email,
    Device,
}

const PROVIDERS: [(Provider, &str); 3] = [
    (Provider::Authenticator, "身份验证器"),
    (Provider::Email, "邮箱"),
    (Provider::YubiKey, "YubiKey"),
];

fn auth_message(status: AuthStatus) -> &'static str {
    match status {
        AuthStatus::InvalidDeviceCode => "设备验证码不正确或已失效。",
        AuthStatus::InvalidTwoFactor => "二步验证码不正确、已用过或已失效。",
        AuthStatus::AlreadySignedIn => "此账户已登录。请在账户菜单切换。",
        AuthStatus::AccountLimit => "已达到五个账户上限。",
        AuthStatus::Cancelled => "操作已取消。",
        AuthStatus::TimedOut => "连接超时，请重试。",
        AuthStatus::RateLimited => "请求过于频繁，请稍后再试。",
        AuthStatus::NetworkError => "无法连接服务器，请检查网络、代理或服务器的 HTTPS 连接后重试。",
        AuthStatus::ServerError => "服务器响应异常或不兼容，请稍后重试。",
        AuthStatus::AuthenticationRejected => {
            "服务器未完成身份验证，请检查账号和服务器设置后重试。"
        }
        _ => "登录未完成，请检查账号、服务器地址和网络后重试。",
    }
}
