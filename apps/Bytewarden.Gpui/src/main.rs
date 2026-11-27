#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod backup;
mod client;
mod clipboard;
mod detail_tools;
mod editor;
mod favicon;
mod folders;
mod generator;
mod generator_preferences;
mod input;
mod motion;
mod passkeys;
mod sends;
mod sensitive_text;
mod settings;
mod startup;
mod theme;
mod timeout;
mod totp_qr;
mod tray;
mod ui;
mod vault_list;
#[cfg(all(test, feature = "test-support"))]
mod workflow_tests;

use std::{
    collections::HashSet,
    rc::Rc,
    time::{Duration, Instant},
};

use bytewarden_core::desktop::{self, Item, ItemAction, Request};
use client::{Client, Form};
use generator::Generator;
use gpui::{
    App, Application, Context, FocusHandle, KeyBinding, MouseButton, TitlebarOptions,
    UniformListScrollHandle, Window, WindowOptions, actions, div, prelude::*, px, size,
    uniform_list,
};
#[cfg(test)]
use gpui::{Bounds, WindowBounds};
use motion::{CONTENT_TIME, RAIL, Sidebar, Tween};
use vault_list::{Filter, Sort};
use zeroize::Zeroizing;

actions!(
    bytewarden,
    [
        ToggleSidebar,
        FocusSearch,
        Submit,
        Sync,
        Lock,
        NextInput,
        PrevInput,
        NewItem,
        EditItem,
        SaveEditor,
        OpenCommandPalette,
        QuickCopyUsername,
        QuickCopyPassword,
        QuickCopyTotp,
        ShowShortcuts,
        DismissNavigation,
        ToggleSelection,
        SelectVisible,
        ClearSelection,
        VaultNavUp,
        VaultNavDown,
        VaultDelete
    ]
);

struct VaultPreview {
    client: Client,
    window_handle: gpui::AnyWindowHandle,
    page: Page,
    folders: folders::Folders,
    sends: sends::SendPage,
    backup: backup::Backup,
    settings: settings::Settings,
    timeouts: timeout::Timeouts,
    page_motion: Option<Tween>,
    query: String,
    projection: Option<Rc<[usize]>>,
    search_index: vault_list::Index,
    index_generation: Option<u64>,
    membership: Option<(usize, bool)>,
    scroll: UniformListScrollHandle,
    filter: Filter,
    selected: Option<usize>,
    sort: Sort,
    focus: FocusHandle,
    sidebar: Sidebar,
    animate: bool,
    list_motion: Option<Tween>,
    detail_motion: Option<Tween>,
    lock_queued: bool,
    clone_pending: bool,
    closing: bool,
    generator: Generator,
    favicon_cache: favicon::FaviconCache,
    tray: Option<tray::Tray>,
    single_instance: Option<tray::SingleInstance>,
    passkeys: passkeys::PasskeyState,
    clipboard: clipboard::Clipboard,
    clipboard_generation: u64,
    confirm_attachment: Option<String>,
    selection: VaultSelection,
    selected_item_id: Option<String>,
    pending_quick_copy: Option<PendingQuickCopy>,
    selection_password: gpui::Entity<input::Input>,
    ui: ui::UiState,
}

#[derive(Default)]
struct VaultSelection {
    active: bool,
    ids: HashSet<String>,
    account: Option<String>,
    pending: Option<SelectionOperation>,
    needs_password: bool,
    move_picker: bool,
    empty_trash: bool,
    folder_scroll: UniformListScrollHandle,
    // Only a fresh writable snapshot can reopen writes after uncertain/partial results.
    blocked_generation: Option<u64>,
}

impl VaultSelection {
    fn clear(&mut self) {
        self.active = false;
        self.ids.clear();
        self.pending = None;
        self.needs_password = false;
        self.move_picker = false;
        self.empty_trash = false;
    }

    fn retain_visible<'a>(&mut self, ids: impl Iterator<Item = &'a str>) {
        if self.ids.is_empty() {
            return;
        }
        let visible: HashSet<&str> = ids.collect();
        self.ids.retain(|id| visible.contains(id.as_str()));
    }

    fn confirmed(&mut self, ids: &[String]) {
        for id in ids {
            self.ids.remove(id);
        }
    }
}

#[derive(Clone, Copy)]
enum SelectionAction {
    Move,
    Item(ItemAction),
}

impl SelectionAction {
    fn label(self) -> &'static str {
        match self {
            Self::Move => "移动",
            Self::Item(ItemAction::Favorite) => "收藏",
            Self::Item(ItemAction::Unfavorite) => "取消收藏",
            Self::Item(ItemAction::Archive) => "归档",
            Self::Item(ItemAction::Unarchive) => "取消归档",
            Self::Item(ItemAction::Trash) => "移至回收站",
            Self::Item(ItemAction::Restore) => "还原",
            Self::Item(ItemAction::Delete) => "永久删除",
        }
    }

    fn allowed(self, item: &Item) -> bool {
        !item.restricted
            && match self {
                Self::Item(ItemAction::Restore | ItemAction::Delete) => item.deleted,
                _ => !item.deleted,
            }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuickCopyAction {
    Username,
    Password,
    Totp,
}

struct PendingQuickCopy {
    pub(crate) item_id: String,
    pub(crate) action: QuickCopyAction,
}

#[derive(Clone)]
struct SelectionOperation {
    action: SelectionAction,
    items: Vec<(String, String)>,
    folder: Option<String>,
    empty_trash: bool,
}

#[cfg(test)]
#[path = "main_selection_tests.rs"]
mod main_selection_tests;
#[cfg(test)]
#[path = "main_shutdown_tests.rs"]
mod main_shutdown_tests;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Vault,
    Folders,
    Sends,
    Backup,
    Generator,
    Settings,
    About,
}

fn eligible_trash_ids(items: &[Item]) -> Vec<String> {
    let mut ids = items
        .iter()
        .filter(|item| item.deleted && !item.restricted)
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

fn should_retry_clipboard_clear(cleared: bool, now: Instant, deadline: Instant) -> bool {
    !cleared && now < deadline
}

impl VaultPreview {
    fn initial_state(
        window: &Window,
        settings: settings::Settings,
        focus: gpui::FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let sort = settings.sort;
        let list_width = settings.list_width;
        let ui_state = ui::UiState::new(list_width, cx);
        Self {
            client: Client::new(cx),
            window_handle: window.window_handle(),
            page: Page::Vault,
            folders: folders::Folders::default(),
            sends: sends::SendPage::default(),
            backup: backup::Backup::new(cx),
            settings,
            timeouts: timeout::Timeouts::default(),
            page_motion: None,
            query: String::new(),
            projection: None,
            search_index: vault_list::Index::default(),
            index_generation: None,
            membership: None,
            scroll: UniformListScrollHandle::new(),
            filter: Filter::All,
            selected: None,
            sort,
            focus,
            sidebar: Sidebar::default(),
            animate: motion::effects_enabled(),
            list_motion: None,
            detail_motion: None,
            lock_queued: false,
            clone_pending: false,
            closing: false,
            generator: Generator::new(cx),
            favicon_cache: favicon::FaviconCache::default(),
            tray: None,
            single_instance: None,
            passkeys: passkeys::PasskeyState::default(),
            clipboard: clipboard::Clipboard::default(),
            clipboard_generation: 0,
            confirm_attachment: None,
            selection: VaultSelection::default(),
            selected_item_id: None,
            pending_quick_copy: None,
            selection_password: cx.new(|cx| input::Input::new("主密码", true, cx)),
            ui: ui_state,
        }
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        ui::brand::apply_window_icon(window);
        let mut view = Self::initial_state(window, settings::Settings::load(), focus, cx);
        tray::apply_screen_capture_protection(window, view.settings.prevent_screenshots);
        view.client
            .search
            .update(cx, |input, _| input.set_borderless(true));
        cx.observe(&view.ui.palette_input, |this, input, cx| {
            let input = input.read(cx);
            if input.committed() && this.ui.palette_query != input.value() {
                this.ui.palette_query = input.value().to_string();
                this.ui.palette_selected = 0;
                cx.notify();
            }
        })
        .detach();
        cx.observe(&view.client.search, |this, input, cx| {
            let input = input.read(cx);
            if input.committed() && this.query != input.value() {
                this.query = input.value().to_string();
                this.invalidate_query();
                cx.notify();
            }
        })
        .detach();
        window
            .observe_window_appearance(|window, _| window.refresh())
            .detach();
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |this, cx| {
                this.settings.capture_window_bounds(window);
                this.settings.save_silent();
                let tray_registered = this.tray.as_ref().is_some_and(tray::Tray::is_registered);
                let exit_requested = this
                    .tray
                    .as_mut()
                    .is_some_and(tray::Tray::take_exit_requested);
                match tray::close_decision(
                    this.settings.keep_in_tray,
                    tray_registered,
                    exit_requested,
                    this.closing,
                ) {
                    tray::CloseDecision::HideToTray => {
                        if let Some(tray) = this.tray.as_ref() {
                            tray.hide();
                        }
                        false
                    }
                    tray::CloseDecision::ShutDown => this.begin_shutdown(window, cx),
                    tray::CloseDecision::KeepOpen => false,
                }
            })
            .unwrap_or(true)
        });
        view.initialize_timeouts(window, cx);
        view.observe_generator(cx);
        view.start_passkey_provider(window, cx);
        view.initialize(cx);
        view
    }

    fn begin_shutdown(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.closing {
            return false;
        }
        self.closing = true;
        self.stop_timeouts();
        self.stop_passkey_provider(cx);
        self.favicon_cache.clear();
        self.selection.clear();
        self.selection_password
            .update(cx, |field, cx| field.set("", cx));
        self.cancel_pending(cx);
        self.clear_account_view(cx);
        self.client.notice = "正在取消操作并清理剪贴板，完成后退出…".into();
        let clock = cx.background_executor().clone();
        cx.spawn_in(window, async move |view, app| {
            loop {
                let drained = view
                    .update(app, |this, _| this.client.busy.is_none())
                    .unwrap_or(true);
                if drained {
                    break;
                }
                clock.timer(Duration::from_millis(100)).await;
            }
            // Revoke the core session as soon as all cancelled operations have
            // drained. Clipboard contention must never keep vault keys alive or
            // prevent the application from closing indefinitely.
            clock.spawn(async { desktop::shutdown() }).await;
            // Release the single-instance guard now that the core cache is closed,
            // so relaunching immediately during clipboard cleanup opens cleanly.
            let _ = view.update(app, |this, _| {
                this.single_instance = None;
            });

            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let cleared = view
                    .update(app, |this, _| this.clipboard.clear())
                    .unwrap_or(true);
                if !should_retry_clipboard_clear(cleared, Instant::now(), deadline) {
                    break;
                }
                clock.timer(Duration::from_millis(100)).await;
            }
            app.update(|window, _| window.remove_window()).ok();
        })
        .detach();
        cx.notify();
        false
    }

    fn invalidate_query(&mut self) {
        self.projection = None;
        self.membership = None;
        self.scroll = UniformListScrollHandle::new();
    }

    fn refresh_search_index(&mut self) {
        if self.index_generation != Some(self.client.overview_generation) {
            self.search_index = self
                .client
                .overview
                .as_ref()
                .map_or_else(vault_list::Index::default, |overview| {
                    vault_list::Index::new(&overview.items)
                });
            self.index_generation = Some(self.client.overview_generation);
            self.invalidate_query();
        }
    }

    fn indices(&mut self) -> Rc<[usize]> {
        let account = self.client.active().map(|account| account.id.clone());
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
        if self.selection.account != account || self.client.overview.is_none() {
            self.selection.clear();
            self.selection.account = account;
            self.selection.blocked_generation = None;
        }
        if self.selection.blocked_generation.is_some_and(|generation| {
            generation != self.client.overview_generation
                && self
                    .client
                    .overview
                    .as_ref()
                    .is_some_and(|overview| !overview.read_only)
        }) {
            self.selection.blocked_generation = None;
        }
        self.refresh_search_index();
        let projection_changed = self.projection.is_none();
        let indices = self
            .projection
            .get_or_insert_with(|| {
                self.client.overview.as_ref().map_or_else(
                    || Rc::from([]),
                    |overview| {
                        self.search_index
                            .visible(
                                &overview.items,
                                self.filter,
                                self.client.selected_folder.as_deref(),
                                self.sort,
                                &self.query,
                            )
                            .into()
                    },
                )
            })
            .clone();
        if projection_changed
            && self.selection.blocked_generation.is_none()
            && let Some(overview) = &self.client.overview
        {
            self.selection.retain_visible(
                indices
                    .iter()
                    .filter_map(|index| overview.items.get(*index).map(|item| item.id.as_str())),
            );
        }
        if let Some(selected) = self.selected {
            let visible = match self.membership {
                Some((checked, visible)) if checked == selected => visible,
                _ => {
                    let visible = indices.contains(&selected);
                    self.membership = Some((selected, visible));
                    visible
                }
            };
            if !visible && self.client.form.is_none() {
                self.close_detail();
            }
        }
        indices
    }

    fn selection_ready(&self) -> bool {
        self.page == Page::Vault
            && self.write_available()
            && !self.lock_queued
            && self.selection.blocked_generation.is_none()
    }

    fn toggle_selection(&mut self, cx: &mut Context<Self>) {
        if (!self.selection.active && !self.selection_ready())
            || !self.client.ready()
            || self.selection.pending.is_some()
        {
            return;
        }
        let active = !self.selection.active;
        self.close_detail();
        self.selection.clear();
        self.selection.active = active;
        self.selection_password
            .update(cx, |field, cx| field.set("", cx));
        cx.notify();
    }

    fn select_visible(&mut self, cx: &mut Context<Self>) {
        if !self.selection_ready() || !self.selection.active || self.selection.pending.is_some() {
            return;
        }
        let indices = self.indices();
        if let Some(overview) = &self.client.overview {
            self.selection.ids = indices
                .iter()
                .filter_map(|index| {
                    overview
                        .items
                        .get(*index)
                        .filter(|item| !item.restricted)
                        .map(|item| item.id.clone())
                })
                .collect();
        }
        self.selection.move_picker = false;
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.client.ready() && self.selection.pending.is_none() {
            self.selection.ids.clear();
            self.selection.move_picker = false;
            cx.notify();
        }
    }

    fn toggle_selected_id(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.selection_ready() || !self.selection.active || self.selection.pending.is_some() {
            return;
        }
        if !self.selection.ids.remove(id) {
            self.selection.ids.insert(id.to_owned());
        }
        self.selection.move_picker = false;
        cx.notify();
    }

    fn empty_trash(&mut self, cx: &mut Context<Self>) {
        if self.page != Page::Vault || !self.selection_ready() {
            return;
        }
        let Some(overview) = self.client.overview.as_ref() else {
            return;
        };
        let ids = eligible_trash_ids(&overview.items);
        if ids.is_empty() {
            self.client.notice = "回收站中没有可操作的条目。".into();
            cx.notify();
            return;
        }
        let Some(account) = self.client.active().map(|account| account.id.clone()) else {
            return;
        };
        self.filter = Filter::Trash;
        self.client.selected_folder = None;
        self.client.search.update(cx, |input, cx| input.set("", cx));
        self.query.clear();
        self.invalidate_query();
        self.close_detail();
        self.selection.clear();
        self.selection.active = true;
        self.selection.account = Some(account);
        self.selection.ids = ids.into_iter().collect();
        self.selection.empty_trash = true;
        self.begin_selection_action(SelectionAction::Item(ItemAction::Delete), None, cx);
    }

    fn begin_selection_action(
        &mut self,
        action: SelectionAction,
        folder: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.selection_ready() || !self.selection.active || self.selection.pending.is_some() {
            return;
        }
        let count = self.selection.ids.len();
        let empty_trash = self.selection.empty_trash
            && matches!(action, SelectionAction::Item(ItemAction::Delete));
        if count == 0 || (count > 500 && !empty_trash) {
            self.client.notice = if empty_trash {
                "回收站中没有可操作的条目。".into()
            } else {
                "请选择 1 至 500 个条目进行批量操作。".into()
            };
            cx.notify();
            return;
        }
        let Some(overview) = self.client.overview.as_ref() else {
            return;
        };
        let selected = overview
            .items
            .iter()
            .filter(|item| self.selection.ids.contains(&item.id))
            .collect::<Vec<_>>();
        if selected.len() != count || selected.iter().any(|item| !action.allowed(item)) {
            self.client.notice = "所选条目不支持此操作，请检查条目状态和权限。".into();
            cx.notify();
            return;
        }
        self.selection.needs_password =
            !matches!(action, SelectionAction::Move) && selected.iter().any(|item| item.reprompt);
        self.selection.pending = Some(SelectionOperation {
            action,
            items: selected
                .iter()
                .map(|item| (item.id.clone(), item.revision_date.clone()))
                .collect(),
            folder,
            empty_trash,
        });
        self.client.show_accounts = false;
        self.selection.move_picker = false;
        self.selection_password
            .update(cx, |field, cx| field.set("", cx));
        cx.notify();
    }

    fn cancel_selection_action(&mut self, cx: &mut Context<Self>) {
        if !self.client.ready() {
            self.cancel_pending(cx);
            return;
        }
        self.selection.pending = None;
        if self.selection.empty_trash {
            self.selection.clear();
        }
        self.selection.needs_password = false;
        self.selection_password
            .update(cx, |field, cx| field.set("", cx));
        cx.notify();
    }

    fn submit_selection_action(&mut self, cx: &mut Context<Self>) {
        if !self.selection_ready() || !self.selection_password.read(cx).committed() {
            return;
        }
        let Some(operation) = self.selection.pending.clone() else {
            return;
        };
        let password = self
            .selection_password
            .update(cx, |field, cx| field.take(cx));
        let req = Request::begin();
        self.run(
            "正在更新所选条目",
            Some(req),
            cx,
            move || {
                let ids = operation.items.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
                match operation.action {
                    SelectionAction::Move => match req.move_items(&ids, operation.folder.as_deref()) {
                        Ok(report) => (ids, Some(report.overview), None),
                        Err(error) => (Vec::new(), None, Some(error)),
                    },
                    SelectionAction::Item(action @ (ItemAction::Trash | ItemAction::Restore | ItemAction::Delete)) => {
                        let mut confirmed = Vec::new();
                        let mut overview = None;
                        let batches = if operation.empty_trash { 500 } else { ids.len().max(1) };
                        for batch in ids.chunks(batches) {
                            match req.bulk_action(batch, action, &password) {
                                Ok(report) => {
                                    confirmed.extend(batch.iter().cloned());
                                    overview = Some(report.overview);
                                }
                                Err(error) => return (confirmed, overview, Some(error)),
                            }
                        }
                        (confirmed, overview, None)
                    }
                    SelectionAction::Item(action) => {
                        // Metadata actions have no core batch endpoint. Commit each once,
                        // stop at the first failure, and report only confirmed successes.
                        let mut confirmed = Vec::new();
                        let mut overview = None;
                        for (id, revision) in operation.items {
                            match req.action(&id, action, &revision, &password) {
                                Ok(snapshot) => {
                                    confirmed.push(id);
                                    overview = Some(snapshot);
                                }
                                Err(error) => return (confirmed, overview, Some(error)),
                            }
                        }
                        (confirmed, overview, None)
                    }
                }
            },
            |this, (confirmed, overview, error), cx| {
                this.selection_password.update(cx, |field, cx| field.set("", cx));
                if this.lock_queued || this.client.overview.is_none() {
                    this.selection.clear();
                    return;
                }
                if let Some(overview) = overview {
                    this.client.set_overview(overview);
                    this.invalidate_query();
                }
                this.selection.confirmed(&confirmed);
                if let Some(error) = error {
                    let partial = !confirmed.is_empty();
                    let reverify = !partial && matches!(error.code(), "passwordRequired" | "wrongPassword");
                    let uncertain = partial || selection_requires_sync(error.code());
                    if uncertain {
                        // Existing editor/detail paths also respect read_only. Sync installs
                        // a fresh overview before any further writes become available.
                        if let Some(overview) = this.client.overview.as_mut() {
                            overview.read_only = true;
                        }
                        this.selection.blocked_generation = Some(this.client.overview_generation);
                    }
                    if reverify {
                        this.selection.needs_password = true;
                    } else {
                        this.selection.pending = None;
                        this.selection.needs_password = false;
                    }
                    this.client.notice = if uncertain {
                        format!("已确认完成 {} 个条目，其余选择已保留。请先同步核对结果，不要重复提交。", confirmed.len())
                    } else {
                        format!("{} 所选条目已保留。", client::status_text(&error))
                    };
                } else {
                    let label = this.selection.pending.as_ref().map_or("更新", |operation| operation.action.label());
                    this.client.notice = format!("已{label} {} 个条目。", confirmed.len());
                    this.selection.clear();
                }
            },
        );
    }

    fn selection_modal_snapshot(&self) -> Option<ui::state::ModalClosingSnapshot> {
        let operation = self.selection.pending.as_ref()?;
        let label = operation.action.label();
        Some(ui::state::ModalClosingSnapshot {
            title: format!("确定要{label}所选的 {} 个条目？", operation.items.len()),
            subtitle: if matches!(operation.action, SelectionAction::Item(ItemAction::Delete)) {
                "此操作不可撤销，永久删除的条目无法恢复。".into()
            } else {
                "确认后将应用到所选条目。".into()
            },
            danger: matches!(operation.action, SelectionAction::Item(ItemAction::Delete)),
            action_label: label,
        })
    }

    fn render_selection_confirmation(&self, cx: &mut Context<Self>) -> gpui::Div {
        let pal = theme::palette();
        let operation = self
            .selection
            .pending
            .as_ref()
            .expect("selection confirmation visible");
        let label = operation.action.label();
        let is_danger = matches!(operation.action, SelectionAction::Item(ItemAction::Delete));
        ui::components::elevated_card()
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(ui::tokens::space::M))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .size(px(34.))
                            .rounded(px(ui::tokens::radius::PILL))
                            .bg(pal.tint(if is_danger {
                                pal.danger
                            } else {
                                pal.accent_fill
                            }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme::hex(if is_danger {
                                pal.danger
                            } else {
                                pal.accent_text
                            }))
                            .child(ui::components::glyph_icon(
                                if is_danger {
                                    ui::tokens::glyph::DELETE
                                } else {
                                    ui::tokens::glyph::CHECK_BOX
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
                                    .text_size(px(ui::tokens::font::BODY_LG))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme::hex(pal.text))
                                    .child(format!(
                                        "确定要{label}所选的 {} 个条目？",
                                        operation.items.len()
                                    )),
                            )
                            .when(is_danger, |el| {
                                el.child(
                                    div()
                                        .text_size(px(ui::tokens::font::CAPTION))
                                        .text_color(theme::hex(pal.danger))
                                        .child("此操作不可撤销，永久删除的条目无法恢复。"),
                                )
                            }),
                    ),
            )
            .when(operation.empty_trash, |panel| {
                panel.child(ui::components::info_bar(
                    ui::toast::ToastKind::Warning,
                    "回收站超过 500 项时将分批处理；任一批次未确认时会停止并要求同步核对，不会自动重试。",
                ))
            })
            .when(matches!(operation.action, SelectionAction::Move), |panel| {
                let folder = operation.folder.as_deref().map_or("无文件夹", |id| {
                    self.folders.name(id, self.client.overview.as_ref())
                });
                panel.child(
                    div()
                        .p(px(10.))
                        .rounded(px(ui::tokens::radius::CONTROL))
                        .bg(theme::hex(pal.layer))
                        .border_1()
                        .border_color(theme::hex(pal.border_subtle))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(ui::tokens::font::CAPTION))
                        .text_color(theme::hex(pal.text_secondary))
                        .child(ui::components::glyph_icon(ui::tokens::glyph::FOLDER, 13.))
                        .child(format!("目标文件夹：{folder}")),
                )
            })
            .when(
                matches!(
                    operation.action,
                    SelectionAction::Item(
                        ItemAction::Favorite
                            | ItemAction::Unfavorite
                            | ItemAction::Archive
                            | ItemAction::Unarchive
                    )
                ),
                |panel| {
                    panel.child(
                        div()
                            .text_size(px(ui::tokens::font::CAPTION))
                            .text_color(theme::hex(pal.text_muted))
                            .child("条目将逐个更新；遇到错误即停止，并保留尚未确认完成的选择。"),
                    )
                },
            )
            .when(self.selection.needs_password, |panel| {
                panel
                    .child(
                        div()
                            .text_size(px(ui::tokens::font::CAPTION))
                            .text_color(theme::hex(pal.text_secondary))
                            .child("请验证主密码后继续。"),
                    )
                    .child(self.selection_password.clone())
            })
            .child(
                div()
                    .pt(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(self.client.ready(), |bar| {
                        bar.child(
                            ui::components::btn(
                                "confirm-selection",
                                if is_danger {
                                    ui::components::BtnVariant::Danger
                                } else {
                                    ui::components::BtnVariant::Primary
                                },
                            )
                            .flex_1()
                            .h(px(36.))
                            .focusable()
                            .tab_index(0)
                            .child(ui::components::glyph_icon(ui::tokens::glyph::CHECK, 13.))
                            .child(label)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.submit_selection_action(cx)
                            }))
                            .on_action(cx.listener(|this, _: &Submit, _, cx| {
                                cx.stop_propagation();
                                this.submit_selection_action(cx);
                            }))
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    cx.stop_propagation();
                                    this.submit_selection_action(cx);
                                }
                            })),
                        )
                    })
                    .child(
                        ui::components::btn(
                            "cancel-selection-action",
                            ui::components::BtnVariant::Secondary,
                        )
                        .h(px(36.))
                        .px(px(16.))
                        .focusable()
                        .tab_index(0)
                        .child(if self.client.ready() {
                            "取消"
                        } else {
                            "取消请求"
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_selection_action(cx)))
                        .on_action(cx.listener(|this, _: &Submit, _, cx| {
                            cx.stop_propagation();
                            this.cancel_selection_action(cx);
                        }))
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                this.cancel_selection_action(cx);
                            }
                        })),
                    ),
            )
    }

    fn render_selection_controls(&self, now: Instant, cx: &mut Context<Self>) -> gpui::Div {
        let pal = theme::palette();
        let (sel_p, _) =
            self.ui
                .selection_spring
                .sample_staggered(now, Duration::ZERO, self.animate);
        let mut bar =
            div()
                .relative()
                .top(px(-6. * (1. - sel_p)))
                .opacity(ui::spring::clamp_opacity(sel_p))
                .p(px(8.))
                .rounded(px(ui::tokens::radius::CONTROL))
                .bg(pal.tint(pal.accent_fill))
                .border_1()
                .border_color(pal.tint_border(pal.accent_fill))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(div().text_color(theme::hex(pal.accent_text)).child(
                                    ui::components::glyph_icon(ui::tokens::glyph::CHECK_BOX, 13.),
                                ))
                                .child(
                                    div()
                                        .text_size(px(ui::tokens::font::CAPTION))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme::hex(pal.text))
                                        .child(format!("已选 {} 项", self.selection.ids.len())),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .when(
                                    self.selection_ready() && self.selection.pending.is_none(),
                                    |row| {
                                        row.child(
                                            ui::components::btn(
                                                "select-visible",
                                                ui::components::BtnVariant::Subtle,
                                            )
                                            .h(px(24.))
                                            .px(px(8.))
                                            .text_size(px(ui::tokens::font::MICRO))
                                            .focusable()
                                            .tab_index(0)
                                            .child("全选")
                                            .tooltip(|_, cx| {
                                                ui::components::make_tooltip(
                                                    "全选当前筛选结果",
                                                    Some("Ctrl+Shift+A"),
                                                    cx,
                                                )
                                            })
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.select_visible(cx)
                                                }),
                                            )
                                            .on_action(cx.listener(|this, _: &Submit, _, cx| {
                                                cx.stop_propagation();
                                                this.select_visible(cx);
                                            })),
                                        )
                                    },
                                )
                                .when(
                                    self.client.ready() && self.selection.pending.is_none(),
                                    |row| {
                                        row.child(
                                            ui::components::btn(
                                                "clear-selection",
                                                ui::components::BtnVariant::Subtle,
                                            )
                                            .h(px(24.))
                                            .px(px(8.))
                                            .text_size(px(ui::tokens::font::MICRO))
                                            .focusable()
                                            .tab_index(0)
                                            .child("清空")
                                            .tooltip(|_, cx| {
                                                ui::components::make_tooltip(
                                                    "清除选择",
                                                    Some("Ctrl+Shift+D"),
                                                    cx,
                                                )
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.clear_selection(cx)
                                            }))
                                            .on_action(cx.listener(|this, _: &Submit, _, cx| {
                                                cx.stop_propagation();
                                                this.clear_selection(cx);
                                            })),
                                        )
                                    },
                                ),
                        ),
                );

        if self.selection_ready()
            && self.selection.pending.is_none()
            && !self.selection.ids.is_empty()
        {
            let mut action_row = div().flex().flex_wrap().items_center().gap(px(4.));
            if self.filter != Filter::Trash {
                action_row = action_row.child(
                    ui::components::btn(
                        "selection-move",
                        if self.selection.move_picker {
                            ui::components::BtnVariant::Tinted
                        } else {
                            ui::components::BtnVariant::Secondary
                        },
                    )
                    .h(px(26.))
                    .px(px(8.))
                    .text_size(px(ui::tokens::font::MICRO))
                    .focusable()
                    .tab_index(0)
                    .child(ui::components::glyph_icon(ui::tokens::glyph::FOLDER, 11.))
                    .child("移动")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selection.move_picker = !this.selection.move_picker;
                        this.ui.animate_collapsible(
                            gpui::ElementId::Name("selection-move-picker".into()),
                            this.selection.move_picker,
                            this.animate,
                        );
                        cx.notify();
                    })),
                );
            }
            let actions: &[ItemAction] = if self.filter == Filter::Trash {
                &[ItemAction::Restore, ItemAction::Delete]
            } else {
                &[
                    ItemAction::Favorite,
                    ItemAction::Unfavorite,
                    ItemAction::Archive,
                    ItemAction::Unarchive,
                    ItemAction::Trash,
                ]
            };
            for (index, action) in actions.iter().copied().enumerate() {
                let operation = SelectionAction::Item(action);
                let variant = if matches!(action, ItemAction::Delete | ItemAction::Trash) {
                    ui::components::BtnVariant::Danger
                } else {
                    ui::components::BtnVariant::Secondary
                };
                action_row = action_row.child(
                    ui::components::btn(("selection-action", index), variant)
                        .h(px(26.))
                        .px(px(8.))
                        .text_size(px(ui::tokens::font::MICRO))
                        .focusable()
                        .tab_index(0)
                        .child(operation.label())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.begin_selection_action(operation, None, cx)
                        }))
                        .on_action(cx.listener(move |this, _: &Submit, _, cx| {
                            cx.stop_propagation();
                            this.begin_selection_action(operation, None, cx);
                        }))
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    cx.stop_propagation();
                                    this.begin_selection_action(operation, None, cx);
                                }
                            },
                        )),
                );
            }
            bar = bar.child(action_row);
        } else if self.selection.blocked_generation.is_some() && self.client.ready() {
            bar = bar.child(
                div()
                    .text_size(px(ui::tokens::font::MICRO))
                    .text_color(theme::hex(pal.warning))
                    .child("请同步核对后继续。"),
            );
        }
        bar
    }

    fn render_selection_folders(&self, now: Instant, cx: &mut Context<Self>) -> gpui::Div {
        let move_p = self.ui.collapsible_progress(
            &gpui::ElementId::Name("selection-move-picker".into()),
            self.selection.move_picker,
            now,
            self.animate,
        );
        let count = self
            .client
            .overview
            .as_ref()
            .map_or(0, |overview| overview.folders.len());
        ui::components::elevated_card()
            .relative()
            .top(px(-6. * (1. - move_p)))
            .opacity(ui::spring::clamp_opacity(move_p))
            .h(px(156.))
            .p(px(6.))
            .child(
                uniform_list(
                    "selection-folder-list",
                    count + 1,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        let pal = theme::palette();
                        range
                            .filter_map(|index| {
                                let overview = this.client.overview.as_ref()?;
                                let (id, label) = if index == 0 {
                                    (None, "无文件夹".to_owned())
                                } else {
                                    let folder = overview.folders.get(index - 1)?;
                                    (Some(folder.id.clone()), folder.name.clone())
                                };
                                let keyboard_id = id.clone();
                                let submit_id = id.clone();
                                Some(
                                    div()
                                        .id(("selection-folder", index))
                                        .cursor_pointer()
                                        .h(px(30.))
                                        .w_full()
                                        .px(px(8.))
                                        .rounded(px(ui::tokens::radius::CONTROL))
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_size(px(ui::tokens::font::CAPTION))
                                        .text_color(theme::hex(pal.text))
                                        .hover(move |s| s.bg(theme::hex(pal.hover)))
                                        .focusable()
                                        .tab_index(0)
                                        .child(div().text_color(theme::hex(pal.accent_text)).child(
                                            ui::components::glyph_icon(
                                                ui::tokens::glyph::FOLDER,
                                                12.,
                                            ),
                                        ))
                                        .child(div().flex_1().min_w_0().truncate().child(label))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.begin_selection_action(
                                                SelectionAction::Move,
                                                id.clone(),
                                                cx,
                                            )
                                        }))
                                        .on_action(cx.listener(move |this, _: &Submit, _, cx| {
                                            cx.stop_propagation();
                                            this.begin_selection_action(
                                                SelectionAction::Move,
                                                submit_id.clone(),
                                                cx,
                                            );
                                        }))
                                        .on_key_down(cx.listener(
                                            move |this, event: &gpui::KeyDownEvent, _, cx| {
                                                if matches!(
                                                    event.keystroke.key.as_str(),
                                                    "enter" | "space"
                                                ) {
                                                    cx.stop_propagation();
                                                    this.begin_selection_action(
                                                        SelectionAction::Move,
                                                        keyboard_id.clone(),
                                                        cx,
                                                    );
                                                }
                                            },
                                        )),
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(self.selection.folder_scroll.clone())
                .size_full(),
            )
    }

    fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.animate = self.settings.animations && motion::effects_enabled();
        self.sidebar.toggle(compact(window));
        cx.notify();
    }

    fn transition(&mut self) -> Option<Tween> {
        self.animate = self.settings.animations && motion::effects_enabled();
        self.animate
            .then(|| Tween::new(0., 1., Instant::now(), CONTENT_TIME))
    }

    fn navigation_available(&self) -> bool {
        self.client.form.is_none()
            && self.client.editor.is_none()
            && !self.sends.editing()
            && !self.lock_queued
            && self.selection.pending.is_none()
    }

    fn open_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if !self.navigation_available() {
            return;
        }
        if self.page == Page::Generator && page != Page::Generator {
            self.flush_generator_preferences(cx);
            self.generator.clear_inputs(cx);
            self.generator.clear();
        }
        if self.page == Page::Sends && page != Page::Sends {
            self.sends.clear(cx);
        }
        if self.page == Page::Backup && page != Page::Backup {
            self.backup.clear(cx);
        }
        if self.page == page {
            self.sidebar.dismiss();
            cx.notify();
            return;
        }
        self.sidebar.dismiss();
        self.ui.close_popovers(self.animate);
        self.page_motion = self.transition();
        self.ui.trigger_page_motion(self.animate);
        self.page = page;
        self.selection.clear();
        self.close_detail();
        if page == Page::Sends {
            self.load_sends(cx);
        }
        cx.notify();
    }

    fn copy_protected(&mut self, text: Zeroizing<String>, cx: &mut Context<Self>) {
        self.copy_protected_labeled(text, "generic-copy", "内容", cx);
    }

    pub(crate) fn copy_protected_labeled(
        &mut self,
        text: Zeroizing<String>,
        tag: &str,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.clipboard.copy(&text) {
            self.client.notice = "无法使用安全剪贴板。".into();
            self.ui.toast.show(
                ui::toast::ToastKind::Error,
                "无法使用安全剪贴板",
                Instant::now(),
                self.animate,
            );
            cx.notify();
            return;
        }
        let delay = self.settings.clipboard_seconds;
        let duration = Duration::from_secs(delay);
        self.client.notice = format!("已复制{label}，{delay} 秒后清除本应用的剪贴板内容。");
        self.ui.mark_copied(tag, label, duration, self.animate);
        self.schedule_clipboard_clear(duration, cx);
        if self.settings.minimize_after_copy {
            let window_handle = self.window_handle;
            cx.defer(move |app| {
                let _ = window_handle.update(app, |_, window, _| window.minimize_window());
            });
        }
        cx.notify();
    }

    fn clear_clipboard(&mut self, cx: &mut Context<Self>) {
        if !self.clipboard.clear() {
            self.schedule_clipboard_clear(std::time::Duration::ZERO, cx);
        } else {
            self.clipboard_generation = self.clipboard_generation.wrapping_add(1);
        }
    }

    fn schedule_clipboard_clear(&mut self, delay: std::time::Duration, cx: &mut Context<Self>) {
        self.clipboard_generation = self.clipboard_generation.wrapping_add(1);
        let generation = self.clipboard_generation;
        let clock = cx.background_executor().clone();
        cx.spawn(async move |view, app| {
            clock.timer(delay).await;
            let mut attempts = 0;
            while view
                .update(app, |this, _| {
                    this.clipboard_generation == generation && !this.clipboard.clear()
                })
                .unwrap_or(false)
            {
                let interval = if attempts < 12 { 250 } else { 1000 };
                attempts = u8::min(attempts + 1, 12);
                clock
                    .timer(std::time::Duration::from_millis(interval))
                    .await;
            }
        })
        .detach();
    }
}

fn selection_requires_sync(code: &str) -> bool {
    matches!(
        code,
        "cancelled" | "timedOut" | "writeUncertain" | "conflict" | "notSynced"
    )
}

fn compact(window: &Window) -> bool {
    motion::nav_compact(window.viewport_size().width)
}

fn minimal(window: &Window) -> bool {
    motion::nav_minimal(window.viewport_size().width)
}

const SHELL: u32 = 0x202020;
const BG: u32 = 0x282828;
const PANEL: u32 = 0x2d2d2d;
const SELECTED: u32 = 0x3a3a3a;
const BORDER: u32 = 0x404040;
const TEXT: u32 = 0xf5f5f5;
const MUTED: u32 = 0xbfbfbf;
const DISABLED: u32 = 0x969696;
const ACCENT: u32 = 0x60cdff;

impl Render for VaultPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.favicon_cache.take_retired() {
            let _ = window.drop_image(image);
        }
        theme::apply(self.settings.theme, window);
        let pal = theme::palette();
        if self.closing {
            return div()
                .size_full()
                .bg(pal.layer_bg())
                .text_color(theme::hex(pal.text))
                .font_family(ui::tokens::family::UI)
                .flex()
                .items_center()
                .justify_center()
                .p_5()
                .child(self.client.notice.clone());
        }
        self.animate = self.settings.animations && motion::effects_enabled();
        let compact_navigation = compact(window);
        let minimal_navigation = minimal(window);
        let now = Instant::now();

        // Bridge legacy tween triggers from client/editor into our lively springs.
        if self.detail_motion.take().is_some() {
            self.ui.trigger_detail_motion(self.animate);
        }
        if self.list_motion.take().is_some() {
            self.ui.trigger_list_motion(self.animate);
        }
        if self.page_motion.take().is_some() {
            self.ui.trigger_page_motion(self.animate);
        }

        // Observe state transitions for spring-animated modals, popovers, editor & nav pills.
        let form_active = self.client.form.is_some() || self.selection.pending.is_some();
        if let Some(snapshot) = self
            .selection_modal_snapshot()
            .or_else(|| self.form_modal_snapshot())
        {
            self.ui.closing_modal = Some(snapshot);
        }
        if form_active != self.ui.last_form_active {
            self.ui.last_form_active = form_active;
            if form_active {
                self.ui.modal_spring = if self.animate {
                    ui::spring::Spring::launch(0., 1., ui::spring::SpringConfig::SNAPPY, now)
                } else {
                    ui::spring::Spring::settled(1., ui::spring::SpringConfig::SNAPPY, now)
                };
            } else {
                self.ui.modal_spring.retarget(0., now, self.animate);
            }
        }
        let folder_modal_open = self.folders.has_form();
        if let Some(snapshot) = self.folders.form_snapshot() {
            self.ui.closing_folder_modal = Some(snapshot);
        }
        if folder_modal_open != self.ui.last_folder_modal_open {
            self.ui.last_folder_modal_open = folder_modal_open;
            if folder_modal_open {
                self.ui.folder_modal_spring = if self.animate {
                    ui::spring::Spring::launch(0., 1., ui::spring::SpringConfig::SNAPPY, now)
                } else {
                    ui::spring::Spring::settled(1., ui::spring::SpringConfig::SNAPPY, now)
                };
            } else {
                self.ui.folder_modal_spring.retarget(0., now, self.animate);
            }
        }
        if self.client.show_accounts != self.ui.last_accounts_open {
            self.ui.last_accounts_open = self.client.show_accounts;
            self.ui.accounts_spring.retarget(
                if self.client.show_accounts { 1. } else { 0. },
                now,
                self.animate,
            );
        }
        let editor_open = self.client.editor.is_some();
        if editor_open != self.ui.last_editor_open {
            self.ui.last_editor_open = editor_open;
            if editor_open {
                self.ui.trigger_editor_motion(self.animate);
            }
        }
        if self.selection.active != self.ui.last_selection_active {
            self.ui.last_selection_active = self.selection.active;
            self.ui.selection_spring.retarget(
                if self.selection.active { 1. } else { 0. },
                now,
                self.animate,
            );
        }
        // Convert routine notices into animated floating Toasts.
        self.ui
            .observe_notice(&self.client.notice, now, self.animate);

        // Sample every active animation; request next frame ONLY if something is moving.
        let navigation =
            self.sidebar
                .frame(compact_navigation, minimal_navigation, now, self.animate);
        let width = navigation.width;
        let expanded = self.sidebar.expanded(compact_navigation);
        let narrow_vault =
            motion::vault_compact(window.viewport_size().width, navigation.reserved_width);
        let alpha = ((width - RAIL - 8.) / 72.).clamp(0., 1.);

        let (page_p, page_anim) = self.ui.page_spring.sample(now, self.animate);
        let (list_p, list_anim) = self.ui.list_spring.sample(now, self.animate);
        let (_, detail_anim_base) = self.ui.detail_spring.sample(now, self.animate);
        let (_, detail_anim_tail) =
            self.ui
                .detail_spring
                .sample_staggered(now, Duration::from_millis(70), self.animate);
        let (_, editor_anim) = self.ui.editor_spring.sample(now, self.animate);
        let (modal_p, modal_anim) = self.ui.modal_spring.sample(now, self.animate);
        if !form_active && modal_p <= 0.02 {
            self.ui.closing_modal = None;
        }
        let (folder_modal_p, folder_modal_anim) =
            self.ui.folder_modal_spring.sample(now, self.animate);
        if !folder_modal_open && folder_modal_p <= 0.02 {
            self.ui.closing_folder_modal = None;
        }
        let (accounts_p, accounts_anim) = self.ui.accounts_spring.sample(now, self.animate);
        let (_, menu_anim) = self.ui.item_menu_spring.sample(now, self.animate);
        let (_, new_menu_anim) = self.ui.new_menu_spring.sample(now, self.animate);
        let (ctx_menu_p, ctx_menu_anim) = self.ui.ctx_menu_spring.sample(now, self.animate);
        let (_, editor_gen_anim) = self.ui.editor_gen_spring.sample(now, self.animate);
        let (palette_p, palette_anim) = self.ui.palette_spring.sample(now, self.animate);
        let (shortcuts_p, shortcuts_anim) = self.ui.shortcuts_spring.sample(now, self.animate);
        let (_, fav_anim) = self.ui.favorite_pulse.sample(now, self.animate);
        let (_, copy_anim) = self.ui.copy_pulse.sample(now, self.animate);
        let (_, auth_step_anim) = self.ui.auth_step_spring.sample(now, self.animate);
        let (_, auth_shake_anim) = self.ui.auth_shake_spring.sample(now, self.animate);
        let (_, send_detail_anim) = self.ui.send_detail_spring.sample(now, self.animate);
        let (_, send_editor_anim) = self.ui.send_editor_spring.sample(now, self.animate);
        let (_, folder_nav_anim) = self.ui.folder_nav_spring.sample(now, self.animate);
        let (_, vault_narrow_anim) = self.ui.vault_narrow_spring.sample(now, self.animate);
        let (_, selection_anim) = self.ui.selection_spring.sample(now, self.animate);
        let (_, gen_spin_anim) = self.ui.generator_spin_spring.sample(now, self.animate);
        let (_, gen_result_anim) = self.ui.generator_result_spring.sample(now, self.animate);
        let dynamic_anim = self.ui.sample_dynamic_springs(now, self.animate);
        let busy_spinning = self.client.busy.is_some() && self.animate;

        let (_, toast_anim) = self.ui.toast.sample(now, self.animate);

        self.folders.refresh(
            self.client.overview_generation,
            self.client.overview.as_ref(),
        );
        let indices = self.indices();
        if self.client.overview.is_none() && !self.selection_password.read(cx).value().is_empty() {
            self.selection_password
                .update(cx, |field, cx| field.set("", cx));
        }

        let is_full_auth_form = matches!(self.client.form, Some(Form::SignIn | Form::Unlock));
        let is_modal_form = matches!(
            self.client.form,
            Some(
                Form::HelloEnrollment
                    | Form::Detail
                    | Form::EditorPassword
                    | Form::Reverify(_)
                    | Form::Confirm(_)
            )
        );

        let persistent_banner = self.render_persistent_banner(now, cx);
        let banner_visible = persistent_banner.is_some();
        if banner_visible != self.ui.last_banner_visible {
            self.ui.last_banner_visible = banner_visible;
            self.ui
                .banner_spring
                .retarget(if banner_visible { 1. } else { 0. }, now, self.animate);
        }
        let (_, banner_anim) = self.ui.banner_spring.sample(now, self.animate);

        if navigation.animating
            || page_anim
            || list_anim
            || detail_anim_base
            || detail_anim_tail
            || editor_anim
            || modal_anim
            || folder_modal_anim
            || accounts_anim
            || menu_anim
            || new_menu_anim
            || ctx_menu_anim
            || editor_gen_anim
            || palette_anim
            || shortcuts_anim
            || fav_anim
            || copy_anim
            || auth_step_anim
            || auth_shake_anim
            || send_detail_anim
            || send_editor_anim
            || folder_nav_anim
            || vault_narrow_anim
            || selection_anim
            || banner_anim
            || gen_spin_anim
            || gen_result_anim
            || dynamic_anim
            || toast_anim
            || busy_spinning
        {
            window.request_animation_frame();
        } else if !self.animate
            && let Some(wait) = self.ui.toast.pending_expiry(now)
        {
            // When animations are disabled, still auto-dismiss the toast cleanly after its timer.
            let clock = cx.background_executor().clone();
            cx.spawn(async move |weak, app| {
                clock.timer(wait + Duration::from_millis(20)).await;
                let _ = weak.update(app, |_, cx| cx.notify());
            })
            .detach();
        }

        let compact_chrome = window.viewport_size().width < px(780.);
        let titlebar = self.render_titlebar(minimal_navigation, compact_chrome, now, window, cx);

        let body = if is_full_auth_form {
            div()
                .id("auth-hero-stage")
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_y_scroll()
                .child(
                    div()
                        .w_full()
                        .min_h_full()
                        .px(px(ui::tokens::space::XL))
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(div().flex_1().min_h(px(24.)))
                        .child(
                            div()
                                .relative()
                                .top(px(18. * (1. - modal_p)))
                                .opacity(ui::spring::clamp_opacity(modal_p))
                                .w_full()
                                .max_w(px(440.))
                                .flex_shrink_0()
                                .child(self.render_form(now, cx)),
                        )
                        .child(div().flex_1().min_h(px(24.))),
                )
                .into_any_element()
        } else {
            match self.page {
                Page::Vault => self
                    .render_vault_workspace(indices, narrow_vault, list_p, now, cx)
                    .into_any_element(),
                Page::Folders => self.render_folders(now, cx).into_any_element(),
                Page::Sends => self.render_sends(narrow_vault, now, cx).into_any_element(),
                Page::Backup => div()
                    .id("backup-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(ui::tokens::space::XL))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(760.))
                            .mx_auto()
                            .child(self.render_backup(now, cx)),
                    )
                    .into_any_element(),
                Page::Generator => div()
                    .id("generator-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.generator(now, cx))
                    .into_any_element(),
                Page::Settings => self.render_settings(now, cx).into_any_element(),
                Page::About => div()
                    .id("about-page-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(ui::tokens::space::XL))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(760.))
                            .mx_auto()
                            .child(self.render_about(cx)),
                    )
                    .into_any_element(),
            }
        };

        // Sample toast for final render (immutable view after mutation above).
        let toast_overlay = self.ui.toast.sample(now, self.animate).0.map(|frame| {
            let weak = cx.weak_entity();
            ui::components::toast_pill(&frame, move |_, app| {
                let _ = weak.update(app, |this, cx| {
                    this.ui.toast.dismiss(Instant::now(), this.animate);
                    cx.notify();
                });
            })
        });

        let drawer_scrim_alpha = if navigation.overlay {
            ((width - if minimal_navigation { 0. } else { RAIL })
                / (motion::PANEL - if minimal_navigation { 0. } else { RAIL }).max(1.))
            .clamp(0., 1.)
        } else {
            0.
        };

        div()
            .tab_group()
            .capture_key_down(cx.listener(|this, _, _, cx| this.record_vault_activity(cx)))
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| this.record_vault_activity(cx)))
            .on_scroll_wheel(cx.listener(|this, _, _, cx| this.record_vault_activity(cx)))
            // Resizable Vault List Splitter drag handling.
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.ui.resizing_list {
                    let delta = f32::from(event.position.x) - this.ui.resize_origin_x;
                    let next = (this.ui.resize_start_width + delta)
                        .clamp(ui::tokens::size::LIST_MIN, ui::tokens::size::LIST_MAX);
                    if (next - this.ui.list_width).abs() > 0.5 {
                        this.ui.list_width = next;
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.ui.resizing_list {
                        this.ui.resizing_list = false;
                        if (this.settings.list_width - this.ui.list_width).abs() > 1. {
                            this.settings.list_width = this.ui.list_width;
                            this.save_settings(cx);
                        } else {
                            cx.notify();
                        }
                    }
                }),
            )
            .on_action(|_: &NextInput, window, _| window.focus_next())
            .on_action(|_: &PrevInput, window, _| window.focus_prev())
            .on_action(
                cx.listener(|this, _: &ToggleSidebar, window, cx| this.toggle_sidebar(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenCommandPalette, window, cx| {
                if this.ui.palette_open {
                    this.close_command_palette(window, cx);
                } else {
                    this.open_command_palette(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ShowShortcuts, _, cx| {
                this.toggle_shortcuts_modal(cx);
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                if this.client.overview.is_some()
                    && this.client.form.is_none()
                    && this.page == Page::Vault
                    && this.selection.pending.is_none()
                {
                    this.client.search.read(cx).focus(window);
                } else {
                    this.open_command_palette(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DismissNavigation, window, cx| {
                if this.ui.palette_open {
                    this.close_command_palette(window, cx);
                } else if this.ui.shortcuts_open {
                    this.toggle_shortcuts_modal(cx);
                } else if this.client.show_accounts {
                    this.client.show_accounts = false;
                    cx.notify();
                } else if this.ui.item_menu_open
                    || this.ui.new_menu_open
                    || this.ui.editor_gen_open
                    || this.ui.ctx_menu.is_some()
                {
                    this.ui.close_popovers(this.animate);
                    cx.notify();
                } else if this.selection.pending.is_some() {
                    this.cancel_selection_action(cx);
                } else if this.client.form.is_some() {
                    this.close_form(cx);
                    this.close_detail();
                    this.focus.focus(window);
                    this.session_notice();
                    cx.notify();
                } else if this.passkeys.has_modal() {
                    this.clear_passkey_modal(true, cx);
                } else if this.folders.has_form() && this.client.ready() {
                    this.folders.dismiss_form();
                    cx.notify();
                } else if this.sidebar.dismiss() {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &Lock, _, cx| this.request_lock(cx)))
            .on_action(cx.listener(|this, _: &Sync, window, cx| this.sync_or_reconnect(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleSelection, _, cx| this.toggle_selection(cx)))
            .on_action(cx.listener(|this, _: &SelectVisible, _, cx| this.select_visible(cx)))
            .on_action(cx.listener(|this, _: &ClearSelection, _, cx| this.clear_selection(cx)))
            .on_action(cx.listener(|this, _: &VaultNavUp, _, cx| {
                this.step_vault_selection(-1, cx);
            }))
            .on_action(cx.listener(|this, _: &VaultNavDown, _, cx| {
                this.step_vault_selection(1, cx);
            }))
            .on_action(cx.listener(|this, _: &VaultDelete, _, cx| {
                this.delete_selected_vault_item(cx);
            }))
            .on_action(cx.listener(|this, _: &SaveEditor, _, cx| {
                if this.client.editor.is_some() {
                    this.save_editor(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &QuickCopyUsername, _, cx| {
                this.quick_copy_named_field("用户名", "shortcut-user", cx);
            }))
            .on_action(cx.listener(|this, _: &QuickCopyPassword, _, cx| {
                this.quick_copy_named_field("密码", "shortcut-pass", cx);
            }))
            .on_action(cx.listener(|this, _: &QuickCopyTotp, _, cx| {
                this.quick_copy_totp(cx);
            }))
            .on_action(cx.listener(|this, _: &Submit, window, cx| {
                if this.ui.palette_open {
                    this.submit_command_palette(window, cx);
                } else if this.selection.pending.is_some() || this.selection.active {
                    // Selection changes require the explicit confirmation button.
                } else if this.client.editor.is_some() {
                    this.save_editor(cx);
                } else {
                    this.submit(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewItem, window, cx| {
                if this.page == Page::Vault && !this.selection.active {
                    this.new_item(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &EditItem, _, cx| {
                if this.page == Page::Vault && !this.selection.active {
                    this.edit_item(cx);
                }
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(pal.shell_bg())
            .text_color(theme::hex(pal.text))
            .font_family(ui::tokens::family::UI)
            .text_size(px(ui::tokens::font::BODY))
            .child(titlebar)
            .child(
                div()
                    // Keep automatic focus handling out of the native titlebar:
                    // it prevents the default mouse action and cancels Windows
                    // caption dragging and caption-button presses.
                    .track_focus(&self.focus)
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .when(!minimal_navigation && !is_full_auth_form, |row| {
                        row.child(
                            div()
                                .w(px(navigation.reserved_width))
                                .h_full()
                                .flex_shrink_0()
                                .when(!navigation.overlay, |rail| {
                                    rail.child(self.render_sidebar(
                                        ui::shell::SidebarRender {
                                            width: width.max(RAIL),
                                            alpha,
                                            expanded,
                                            content_offset: 0.,
                                            is_overlay: false,
                                            now,
                                        },
                                        cx,
                                    ))
                                }),
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .min_h_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(ui::tokens::space::S))
                            .bg(pal.layer_bg())
                            .when(!is_full_auth_form, |el| {
                                el.rounded_tl(px(ui::tokens::radius::CARD))
                                    .border_t_1()
                                    .border_l_1()
                                    .border_color(theme::hex(pal.border_subtle))
                            })
                            .p(px(ui::tokens::space::M))
                            .when_some(persistent_banner, |el, banner| el.child(banner))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .min_w_0()
                                    .relative()
                                    .top(px(10. * (1. - page_p)))
                                    .opacity(ui::spring::clamp_opacity(0.45 + 0.55 * page_p))
                                    .child(body),
                            ),
                    )
                    // Compact / Minimal sidebar overlay drawer with soft backdrop dim.
                    .when(navigation.overlay && !is_full_auth_form, |row| {
                        row.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .left_0()
                                        .right_0()
                                        .bg(pal.scrim(drawer_scrim_alpha))
                                        .occlude()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                if this.sidebar.dismiss() {
                                                    cx.notify();
                                                }
                                            }),
                                        ),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .left_0()
                                        .w(px(width))
                                        .overflow_hidden()
                                        .occlude()
                                        .bg(theme::hex(pal.shell))
                                        .rounded_r(px(ui::tokens::radius::OVERLAY))
                                        .border_r_1()
                                        .border_color(theme::hex(pal.border))
                                        .shadow(ui::components::overlay_shadow())
                                        .child(self.render_sidebar(
                                            ui::shell::SidebarRender {
                                                width: motion::PANEL,
                                                alpha: 1.,
                                                expanded,
                                                content_offset: navigation.content_offset,
                                                is_overlay: true,
                                                now,
                                            },
                                            cx,
                                        )),
                                ),
                        )
                    }),
            )
            // Floating Vault Row Right-Click Context Menu.
            .when(self.ui.ctx_menu.is_some() || ctx_menu_p > 0.02, |root| {
                root.child(self.render_vault_context_menu(ctx_menu_p, window.viewport_size(), cx))
            })
            // Floating Account Switcher Popover.
            .when(self.client.show_accounts || accounts_p > 0.02, |root| {
                root.child(self.render_account_popover(accounts_p, compact_chrome, cx))
            })
            // Floating Modal Overlay for in-vault reprompt / delete confirm / bulk confirmation
            // (mounts immediately on frame 0 when open, and stays mounted with non-sensitive
            // ghost card while exiting smoothly).
            .when(
                is_modal_form
                    || self.selection.pending.is_some()
                    || (modal_p > 0.02 && !is_full_auth_form && self.ui.closing_modal.is_some()),
                |root| {
                    let modal_content = if self.selection.pending.is_some() {
                        self.render_selection_confirmation(cx)
                    } else if is_modal_form {
                        self.render_form(now, cx)
                    } else if let Some(snapshot) = self.ui.closing_modal.as_ref() {
                        Self::render_closing_modal_card(snapshot)
                    } else {
                        div()
                    };
                    root.child(self.render_modal_overlay(
                        modal_p,
                        modal_content,
                        |this, window, cx| {
                            if this.selection.pending.is_some() {
                                this.cancel_selection_action(cx);
                            } else if this.client.form.is_some() {
                                this.close_form(cx);
                                this.close_detail();
                                this.focus.focus(window);
                                this.session_notice();
                                cx.notify();
                            }
                        },
                        cx,
                    ))
                },
            )
            // Centered Spring Modal for Folder create / rename / delete.
            .when(
                folder_modal_open
                    || (folder_modal_p > 0.02 && self.ui.closing_folder_modal.is_some()),
                |root| {
                    let modal_content = if folder_modal_open {
                        self.render_folder_modal(cx)
                    } else if let Some(snapshot) = self.ui.closing_folder_modal.as_ref() {
                        Self::render_closing_modal_card(snapshot)
                    } else {
                        div()
                    };
                    root.child(self.render_modal_overlay(
                        folder_modal_p,
                        modal_content,
                        |this, _, cx| {
                            if this.client.ready() && this.folders.has_form() {
                                this.folders.dismiss_form();
                                cx.notify();
                            }
                        },
                        cx,
                    ))
                },
            )
            // Centered Modal for Windows Native Passkey Provider requests.
            .when(self.passkeys.has_modal(), |root| {
                root.child(self.render_modal_overlay(
                    1.,
                    self.render_passkey_modal(cx),
                    |this, _, cx| {
                        this.clear_passkey_modal(true, cx);
                    },
                    cx,
                ))
            })
            // Command Palette (`Ctrl+K`).
            .when(self.ui.palette_open || palette_p > 0.02, |root| {
                root.child(self.render_command_palette(palette_p, cx))
            })
            // Keyboard Shortcuts Cheat Sheet (`Ctrl+/`).
            .when(self.ui.shortcuts_open || shortcuts_p > 0.02, |root| {
                root.child(self.render_shortcuts_modal(shortcuts_p, cx))
            })
            // Floating bottom-center animated Toast pill.
            .when_some(toast_overlay, |root, toast| root.child(toast))
    }
}

fn bind_shortcuts(cx: &mut App) {
    input::init(cx);
    cx.bind_keys([
        KeyBinding::new("ctrl-b", ToggleSidebar, None),
        KeyBinding::new("ctrl-f", FocusSearch, None),
        KeyBinding::new("ctrl-k", OpenCommandPalette, None),
        KeyBinding::new("ctrl-l", Lock, None),
        KeyBinding::new("ctrl-r", Sync, None),
        KeyBinding::new("enter", Submit, None),
        KeyBinding::new("tab", NextInput, None),
        KeyBinding::new("shift-tab", PrevInput, None),
        KeyBinding::new("ctrl-n", NewItem, None),
        KeyBinding::new("ctrl-e", EditItem, None),
        KeyBinding::new("ctrl-s", SaveEditor, None),
        KeyBinding::new("ctrl-/", ShowShortcuts, None),
        KeyBinding::new("ctrl-shift-u", QuickCopyUsername, None),
        KeyBinding::new("ctrl-shift-p", QuickCopyPassword, None),
        KeyBinding::new("ctrl-shift-t", QuickCopyTotp, None),
        KeyBinding::new("escape", DismissNavigation, None),
        KeyBinding::new("ctrl-shift-s", ToggleSelection, None),
        KeyBinding::new("ctrl-shift-a", SelectVisible, None),
        KeyBinding::new("ctrl-shift-d", ClearSelection, None),
        KeyBinding::new("up", VaultNavUp, None),
        KeyBinding::new("down", VaultNavDown, None),
        KeyBinding::new("delete", VaultDelete, None),
    ]);
}

#[cfg(all(test, windows))]
#[path = "../../../work/gpui-auth-memory-probe.rs"]
mod native_auth_memory_probe;

fn main() {
    let startup_requested =
        std::env::args_os().any(|argument| argument == std::ffi::OsStr::new("--start-in-tray"));
    let Some(single_instance) = tray::SingleInstance::acquire_or_activate(startup_requested) else {
        return;
    };
    let single_instance = std::rc::Rc::new(std::cell::RefCell::new(Some(single_instance)));

    Application::new().run(move |cx: &mut App| {
        bind_shortcuts(cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let initial_bounds = settings::Settings::load().initial_window_bounds(cx);
        let single_instance_for_window = single_instance.clone();
        let window = match cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Bytewarden".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                window_bounds: Some(initial_bounds),
                window_min_size: Some(size(px(420.), px(410.))),
                show: !startup_requested,
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| VaultPreview::new(window, cx));
                let weak = view.downgrade();
                let app = cx.to_async();
                let guard = single_instance_for_window.borrow_mut().take();
                view.update(cx, |this, _cx| {
                    this.single_instance = guard;
                    if this.settings.keep_in_tray {
                        let weak = weak.clone();
                        let app = app.clone();
                        this.tray = tray::Tray::for_window(window, move || {
                            let weak = weak.clone();
                            let app = app.clone();
                            let executor = app.foreground_executor().clone();
                            executor
                                .spawn(async move {
                                    let _ = app.update(|cx| {
                                        let _ = weak.update(cx, |this, cx| this.request_lock(cx));
                                    });
                                })
                                .detach();
                        });
                    }
                });
                view
            },
        ) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("Could not open Bytewarden: {error}");
                cx.quit();
                return;
            }
        };
        let started_hidden = startup_requested
            && window
                .update(cx, |view, window, _cx| {
                    if view.settings.start_hidden
                        && view.settings.keep_in_tray
                        && view.tray.as_ref().is_some_and(tray::Tray::is_registered)
                    {
                        if let Some(tray) = view.tray.as_ref() {
                            tray.hide();
                        }
                        true
                    } else {
                        window.activate_window();
                        false
                    }
                })
                .unwrap_or(false);
        if !started_hidden {
            cx.activate(true);
        }
        let _ = window.update(cx, |view, window, _| {
            if let Some(instance) = &mut view.single_instance {
                instance.attach_window(window);
            }
        });
    });
}
