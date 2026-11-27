//! Central lively UI & animation state owned by `VaultPreview`.
//! Keeps all spring trajectories, popover/modal visibility, toast state,
//! splitter geometry, and command palette state cleanly separated from core vault logic.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{AppContext, Context, ElementId, Entity};

use crate::{VaultPreview, input::Input};

use super::{
    spring::{Spring, SpringConfig},
    toast::{ToastKind, ToastState},
    tokens::size,
};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Appearance,
    Security,
    Windows,
    Data,
    About,
}

impl SettingsTab {
    pub const ALL: [(Self, &'static str, &'static str); 5] = [
        (Self::Appearance, "外观与动效", "\u{e790}"),
        (Self::Security, "安全与超时", "\u{e72e}"),
        (Self::Windows, "托盘与启动", "\u{e7f4}"),
        (Self::Data, "导入与导出", "\u{e8b5}"),
        (Self::About, "关于", "\u{e946}"),
    ];

    pub fn index(self) -> usize {
        match self {
            Self::Appearance => 0,
            Self::Security => 1,
            Self::Windows => 2,
            Self::Data => 3,
            Self::About => 4,
        }
    }
}

/// Non-sensitive visual-only snapshot kept briefly while a dismissed modal
/// completes its spring exit animation. Never stores credentials or Input handles.
#[derive(Clone)]
pub struct ModalClosingSnapshot {
    pub title: String,
    pub subtitle: String,
    pub danger: bool,
    pub action_label: &'static str,
}

/// Unified cross-section sidebar indicator pill state, updated from live GPUI
/// layout bounds during `prepaint` so one pill glides smoothly across every
/// sidebar level (Vault, Folders, Tools, and Bottom pinned items).
pub struct NavPillState {
    pub initialized: bool,
    pub active_key: String,
    pub spring: Spring,
}

impl NavPillState {
    pub fn new(now: Instant) -> Self {
        Self {
            initialized: false,
            active_key: String::new(),
            spring: Spring::settled(0., SpringConfig::SNAPPY, now),
        }
    }
}

pub struct UiState {
    pub toast: ToastState,
    pub nav_pill_main: Rc<RefCell<NavPillState>>,
    pub nav_pill_overlay: Rc<RefCell<NavPillState>>,
    pub page_spring: Spring,
    pub list_spring: Spring,
    pub detail_spring: Spring,
    pub modal_spring: Spring,
    pub closing_modal: Option<ModalClosingSnapshot>,
    pub folder_modal_spring: Spring,
    pub closing_folder_modal: Option<ModalClosingSnapshot>,
    pub last_folder_modal_open: bool,
    pub accounts_spring: Spring,
    pub item_menu_open: bool,
    pub item_menu_spring: Spring,
    pub new_menu_open: bool,
    pub new_menu_spring: Spring,
    pub palette_open: bool,
    pub palette_spring: Spring,
    pub palette_input: Entity<Input>,
    pub palette_query: String,
    pub palette_selected: usize,
    pub shortcuts_open: bool,
    pub shortcuts_spring: Spring,
    pub list_width: f32,
    pub resizing_list: bool,
    pub resize_origin_x: f32,
    pub resize_start_width: f32,
    pub expanded_folders: HashSet<String>,
    pub copied_tag: Option<(String, Instant)>,
    pub copy_pulse: Spring,
    pub favorite_pulse: Spring,
    pub settings_tab: SettingsTab,
    pub meta_expanded: bool,
    pub editor_gen_open: bool,
    pub editor_gen_spring: Spring,
    pub sync_spin_start: Instant,
    pub last_notice_seen: String,
    pub last_form_active: bool,
    pub last_accounts_open: bool,
    pub last_editor_open: bool,
    pub editor_spring: Spring,
    pub auth_step_spring: Spring,
    pub auth_shake_spring: Spring,
    pub send_detail_spring: Spring,
    pub send_editor_spring: Spring,
    pub folder_nav_spring: Spring,
    pub folder_nav_dir: f32,
    pub vault_narrow_spring: Spring,
    pub vault_narrow_dir: f32,
    pub selection_spring: Spring,
    pub last_selection_active: bool,
    pub banner_spring: Spring,
    pub last_banner_visible: bool,
    pub generator_spin_spring: Spring,
    pub generator_result_spring: Spring,
    pub detail_loading_since: Option<Instant>,
    pub toggle_springs: HashMap<ElementId, Spring>,
    pub collapsible_springs: HashMap<ElementId, Spring>,
    pub segmented_springs: HashMap<ElementId, Spring>,
}

impl UiState {
    pub fn new(list_width: f32, cx: &mut Context<VaultPreview>) -> Self {
        let now = Instant::now();
        let palette_input = cx.new(|cx| {
            let mut input = Input::new("搜索条目或输入命令…", false, cx);
            input.set_borderless(true);
            input
        });
        Self {
            toast: ToastState::default(),
            nav_pill_main: Rc::new(RefCell::new(NavPillState::new(now))),
            nav_pill_overlay: Rc::new(RefCell::new(NavPillState::new(now))),
            page_spring: Spring::settled(1., SpringConfig::FLUID, now),
            list_spring: Spring::settled(1., SpringConfig::FLUID, now),
            detail_spring: Spring::settled(1., SpringConfig::FLUID, now),
            modal_spring: Spring::settled(0., SpringConfig::SNAPPY, now),
            closing_modal: None,
            folder_modal_spring: Spring::settled(0., SpringConfig::SNAPPY, now),
            closing_folder_modal: None,
            last_folder_modal_open: false,
            accounts_spring: Spring::settled(0., SpringConfig::FLUID, now),
            item_menu_open: false,
            item_menu_spring: Spring::settled(0., SpringConfig::SNAPPY, now),
            new_menu_open: false,
            new_menu_spring: Spring::settled(0., SpringConfig::SNAPPY, now),
            palette_open: false,
            palette_spring: Spring::settled(0., SpringConfig::SNAPPY, now),
            palette_input,
            palette_query: String::new(),
            palette_selected: 0,
            shortcuts_open: false,
            shortcuts_spring: Spring::settled(0., SpringConfig::SNAPPY, now),
            list_width: list_width.clamp(size::LIST_MIN, size::LIST_MAX),
            resizing_list: false,
            resize_origin_x: 0.,
            resize_start_width: list_width,
            expanded_folders: HashSet::new(),
            copied_tag: None,
            copy_pulse: Spring::settled(0., SpringConfig::QUICK, now),
            favorite_pulse: Spring::settled(0., SpringConfig::SNAPPY, now),
            settings_tab: SettingsTab::Appearance,
            meta_expanded: false,
            editor_gen_open: false,
            editor_gen_spring: Spring::settled(0., SpringConfig::SNAPPY, now),
            sync_spin_start: now,
            last_notice_seen: String::new(),
            last_form_active: false,
            last_accounts_open: false,
            last_editor_open: false,
            editor_spring: Spring::settled(1., SpringConfig::FLUID, now),
            auth_step_spring: Spring::settled(1., SpringConfig::FLUID, now),
            auth_shake_spring: Spring::settled(0., SpringConfig::SNAPPY, now),
            send_detail_spring: Spring::settled(1., SpringConfig::FLUID, now),
            send_editor_spring: Spring::settled(1., SpringConfig::FLUID, now),
            folder_nav_spring: Spring::settled(1., SpringConfig::FLUID, now),
            folder_nav_dir: 1.,
            vault_narrow_spring: Spring::settled(1., SpringConfig::FLUID, now),
            vault_narrow_dir: 1.,
            selection_spring: Spring::settled(0., SpringConfig::FLUID, now),
            last_selection_active: false,
            banner_spring: Spring::settled(0., SpringConfig::FLUID, now),
            last_banner_visible: false,
            generator_spin_spring: Spring::settled(0., SpringConfig::FLUID, now),
            generator_result_spring: Spring::settled(1., SpringConfig::SNAPPY, now),
            detail_loading_since: None,
            toggle_springs: HashMap::new(),
            collapsible_springs: HashMap::new(),
            segmented_springs: HashMap::new(),
        }
    }

    pub fn trigger_page_motion(&mut self, animate: bool) {
        let now = Instant::now();
        self.page_spring = if animate {
            Spring::launch(0., 1., SpringConfig::FLUID, now)
        } else {
            Spring::settled(1., SpringConfig::FLUID, now)
        };
    }

    pub fn trigger_list_motion(&mut self, animate: bool) {
        let now = Instant::now();
        self.list_spring = if animate {
            Spring::launch(0., 1., SpringConfig::FLUID, now)
        } else {
            Spring::settled(1., SpringConfig::FLUID, now)
        };
    }

    pub fn trigger_detail_motion(&mut self, animate: bool) {
        let now = Instant::now();
        self.detail_spring = if animate {
            Spring::launch(0., 1., SpringConfig::FLUID, now)
        } else {
            Spring::settled(1., SpringConfig::FLUID, now)
        };
    }

    pub fn trigger_editor_motion(&mut self, animate: bool) {
        let now = Instant::now();
        self.editor_spring = if animate {
            Spring::launch(0., 1., SpringConfig::FLUID, now)
        } else {
            Spring::settled(1., SpringConfig::FLUID, now)
        };
    }

    pub fn trigger_send_detail_motion(&mut self, animate: bool) {
        let now = Instant::now();
        self.send_detail_spring = if animate {
            Spring::launch(0., 1., SpringConfig::FLUID, now)
        } else {
            Spring::settled(1., SpringConfig::FLUID, now)
        };
    }

    pub fn trigger_send_editor_motion(&mut self, animate: bool) {
        let now = Instant::now();
        self.send_editor_spring = if animate {
            Spring::launch(0., 1., SpringConfig::FLUID, now)
        } else {
            Spring::settled(1., SpringConfig::FLUID, now)
        };
    }

    pub fn trigger_folder_nav(&mut self, forward: bool, animate: bool) {
        let now = Instant::now();
        self.folder_nav_dir = if forward { 1. } else { -1. };
        self.folder_nav_spring = if animate {
            Spring::launch(0., 1., SpringConfig::FLUID, now)
        } else {
            Spring::settled(1., SpringConfig::FLUID, now)
        };
    }

    pub fn trigger_vault_narrow(&mut self, forward: bool, animate: bool) {
        let now = Instant::now();
        self.vault_narrow_dir = if forward { 1. } else { -1. };
        self.vault_narrow_spring = if animate {
            Spring::launch(0., 1., SpringConfig::FLUID, now)
        } else {
            Spring::settled(1., SpringConfig::FLUID, now)
        };
    }

    pub fn trigger_auth_step(&mut self, animate: bool) {
        let now = Instant::now();
        self.auth_step_spring = if animate {
            Spring::launch(0., 1., SpringConfig::FLUID, now)
        } else {
            Spring::settled(1., SpringConfig::FLUID, now)
        };
    }

    pub fn trigger_auth_shake(&mut self, animate: bool) {
        self.auth_shake_spring
            .impulse(130., Instant::now(), animate);
    }

    pub fn trigger_generator_refresh(&mut self, animate: bool) {
        let now = Instant::now();
        if animate {
            let next_target = self.generator_spin_spring.target() + std::f32::consts::PI;
            self.generator_spin_spring.retarget(next_target, now, true);
            self.generator_result_spring = Spring::launch(0., 1., SpringConfig::SNAPPY, now);
        } else {
            self.generator_result_spring = Spring::settled(1., SpringConfig::SNAPPY, now);
        }
    }

    pub fn animate_toggle(&mut self, id: impl Into<ElementId>, to_checked: bool, animate: bool) {
        let id = id.into();
        let now = Instant::now();
        let target = if to_checked { 1. } else { 0. };
        if !animate {
            self.toggle_springs.remove(&id);
            return;
        }
        let from = if to_checked { 0. } else { 1. };
        self.toggle_springs
            .entry(id)
            .and_modify(|spring| spring.retarget(target, now, true))
            .or_insert_with(|| Spring::launch(from, target, SpringConfig::QUICK, now));
    }

    pub fn toggle_progress(
        &self,
        id: &ElementId,
        checked: bool,
        now: Instant,
        animate: bool,
    ) -> f32 {
        if !animate {
            return if checked { 1. } else { 0. };
        }
        self.toggle_springs
            .get(id)
            .map_or(if checked { 1. } else { 0. }, |spring| {
                spring.sample_staggered(now, Duration::ZERO, true).0
            })
    }

    pub fn animate_collapsible(&mut self, id: impl Into<ElementId>, to_open: bool, animate: bool) {
        let id = id.into();
        let now = Instant::now();
        let target = if to_open { 1. } else { 0. };
        if !animate {
            self.collapsible_springs.remove(&id);
            return;
        }
        let from = if to_open { 0. } else { 1. };
        self.collapsible_springs
            .entry(id)
            .and_modify(|spring| spring.retarget(target, now, true))
            .or_insert_with(|| Spring::launch(from, target, SpringConfig::FLUID, now));
    }

    pub fn collapsible_progress(
        &self,
        id: &ElementId,
        open: bool,
        now: Instant,
        animate: bool,
    ) -> f32 {
        if !animate {
            return if open { 1. } else { 0. };
        }
        self.collapsible_springs
            .get(id)
            .map_or(if open { 1. } else { 0. }, |spring| {
                spring.sample_staggered(now, Duration::ZERO, true).0
            })
    }

    pub fn animate_segmented(
        &mut self,
        id: impl Into<ElementId>,
        from_index: usize,
        to_index: usize,
        animate: bool,
    ) {
        let id = id.into();
        let now = Instant::now();
        if !animate || from_index == to_index {
            self.segmented_springs.remove(&id);
            return;
        }
        self.segmented_springs
            .entry(id)
            .and_modify(|spring| spring.retarget(to_index as f32, now, true))
            .or_insert_with(|| {
                Spring::launch(
                    from_index as f32,
                    to_index as f32,
                    SpringConfig::SNAPPY,
                    now,
                )
            });
    }

    pub fn segmented_position(
        &self,
        id: &ElementId,
        active_index: usize,
        now: Instant,
        animate: bool,
    ) -> f32 {
        if !animate {
            return active_index as f32;
        }
        self.segmented_springs
            .get(id)
            .map_or(active_index as f32, |spring| {
                spring.sample_staggered(now, Duration::ZERO, true).0
            })
    }

    /// Sample and prune dynamic control springs. Returns true if any control spring is still active.
    pub fn sample_dynamic_springs(&mut self, now: Instant, animate: bool) -> bool {
        let mut active = false;
        for map in [
            &mut self.toggle_springs,
            &mut self.collapsible_springs,
            &mut self.segmented_springs,
        ] {
            map.retain(|_, spring| {
                let (_, moving) = spring.sample(now, animate);
                if moving {
                    active = true;
                }
                moving
            });
        }
        active
    }

    pub fn mark_copied(
        &mut self,
        tag: impl Into<String>,
        label: &str,
        clear_after: Duration,
        animate: bool,
    ) {
        let now = Instant::now();
        self.copied_tag = Some((tag.into(), now));
        self.copy_pulse.impulse(16., now, animate);
        self.toast.show_clipboard(label, clear_after, now, animate);
    }

    pub fn is_recently_copied(&self, tag: &str, now: Instant) -> bool {
        self.copied_tag.as_ref().is_some_and(|(t, at)| {
            t == tag && now.saturating_duration_since(*at) < Duration::from_millis(1600)
        })
    }

    pub fn close_popovers(&mut self, animate: bool) {
        let now = Instant::now();
        self.item_menu_open = false;
        self.item_menu_spring.retarget(0., now, animate);
        self.new_menu_open = false;
        self.new_menu_spring.retarget(0., now, animate);
        self.editor_gen_open = false;
        self.editor_gen_spring.retarget(0., now, animate);
    }

    /// Classify a status notice written by core/client logic:
    /// returns `(_, false)` if it is a quiet routine status that belongs in a Toast (or hidden),
    /// or `(_, true)` if it deserves a persistent InfoBar.
    pub fn classify_notice(notice: &str) -> (ToastKind, bool) {
        let trimmed = notice.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("保险库已锁定")
            || trimmed.starts_with("请先登录")
            || trimmed.starts_with("已登录，请同步")
            || trimmed == "正在初始化本机保险库…"
        {
            return (ToastKind::Info, false);
        }
        // Persistent banners: offline read-only, errors, conflicts, verification needed, sign-in required.
        let persistent = trimmed.contains("离线")
            || trimmed.contains("失败")
            || trimmed.contains("无法")
            || trimmed.contains("错误")
            || trimmed.contains("核对")
            || trimmed.contains("无效")
            || trimmed.contains("超时")
            || trimmed.contains("取消")
            || trimmed.contains("不允许")
            || trimmed.contains("不一致")
            || trimmed.contains("重新登录")
            || trimmed.contains("验证")
            || trimmed.contains("拒绝")
            || trimmed.contains("未确认");

        let kind = if trimmed.contains("失败")
            || trimmed.contains("错误")
            || trimmed.contains("无法")
            || trimmed.contains("拒绝")
            || trimmed.contains("无效")
        {
            ToastKind::Error
        } else if trimmed.contains("离线")
            || trimmed.contains("核对")
            || trimmed.contains("超时")
            || trimmed.contains("注意")
            || trimmed.contains("重新登录")
        {
            ToastKind::Warning
        } else if trimmed.contains("已保存")
            || trimmed.contains("已复制")
            || trimmed.contains("已同步")
            || trimmed.contains("已加载")
            || trimmed.contains("已解锁")
            || trimmed.contains("已更新")
            || trimmed.contains("已创建")
            || trimmed.contains("已删除")
            || trimmed.contains("已收藏")
            || trimmed.contains("已归档")
            || trimmed.contains("已还原")
            || trimmed.contains("已移动")
            || trimmed.contains("已导入")
            || trimmed.contains("已导出")
            || trimmed.contains("已清除")
        {
            ToastKind::Success
        } else {
            ToastKind::Info
        };
        (kind, persistent)
    }

    /// Observe `notice` changes from legacy client actions and trigger lively Toasts automatically.
    pub fn observe_notice(&mut self, notice: &str, now: Instant, animate: bool) {
        if notice == self.last_notice_seen {
            return;
        }
        self.last_notice_seen = notice.to_owned();
        let trimmed = notice.trim();
        if trimmed.is_empty()
            || trimmed == "正在初始化本机保险库…"
            || trimmed.starts_with("请先登录")
            || trimmed.starts_with("保险库已锁定")
            || trimmed.starts_with("保险库已加载")
            || trimmed.contains("复制")
        {
            // Routine boot/lock notices or copies (which already pop a richer countdown toast) stay quiet.
            return;
        }
        let (kind, persistent) = Self::classify_notice(trimmed);
        if !persistent {
            self.toast.show(kind, trimmed.to_owned(), now, animate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_notice_keeps_routine_lock_non_persistent_and_flags_errors() {
        let (kind, persistent) = UiState::classify_notice("保险库已锁定。");
        assert!(!persistent);
        assert_eq!(kind, ToastKind::Info);

        let (kind, persistent) = UiState::classify_notice("网络连接超时，请核对同步状态。");
        assert!(persistent);
        assert_eq!(kind, ToastKind::Warning);

        let (kind, persistent) = UiState::classify_notice("保存失败，请重试。");
        assert!(persistent);
        assert_eq!(kind, ToastKind::Error);
    }

    #[test]
    fn dynamic_springs_settle_and_auto_prune_to_zero() {
        let start = Instant::now();
        let mut toggles: HashMap<ElementId, Spring> = HashMap::new();
        toggles.insert(
            ElementId::Name("test-toggle".into()),
            Spring::launch(0., 1., SpringConfig::QUICK, start),
        );
        let mid = start + Duration::from_millis(40);
        let (_, moving_mid) = toggles.values_mut().next().unwrap().sample(mid, true);
        assert!(moving_mid);

        let settled = start + Duration::from_secs(2);
        toggles.retain(|_, spring| spring.sample(settled, true).1);
        assert!(toggles.is_empty(), "settled dynamic springs must prune");
    }
}
