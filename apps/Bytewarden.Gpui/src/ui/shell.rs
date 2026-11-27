//! Reorganized Fluent 2 Sidebar (with sliding spring indicator pill & collapsible Folder Tree),
//! floating Account Switcher Popover, Keyboard Shortcuts Modal, and Modal Shell.

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    Bounds, Context, Corners, Div, ElementId, MouseButton, Stateful, canvas, div, fill, point,
    prelude::*, px, size,
};

use crate::{
    Page, VaultPreview,
    theme::{self, hex},
    vault_list::Filter,
};

use super::{
    components::{
        BtnVariant, avatar, badge, btn, elevated_card, glyph_icon, icon_btn, make_tooltip,
        rotating_chevron,
    },
    spring::clamp_opacity,
    state::{ModalClosingSnapshot, SettingsTab},
    tokens::{family, font, glyph, radius, space},
};

/// Render one sidebar nav row with icon slot aligned across rail/expanded/drawer modes.
fn sidebar_item(
    id: impl Into<gpui::ElementId>,
    active: bool,
    label: &'static str,
    code: &'static str,
    alpha: f32,
    count: Option<usize>,
) -> Stateful<Div> {
    let pal = theme::palette();
    div()
        .id(id)
        .cursor_pointer()
        .relative()
        .h(px(36.))
        .rounded(px(radius::CONTROL))
        .flex()
        .items_center()
        .gap(px(4.))
        .when(active, |el| el.bg(hex(pal.selected)))
        .when(!active, |el| el.hover(move |s| s.bg(hex(pal.hover))))
        .active(|s| s.top(px(0.5)))
        .child(
            div()
                .size(px(38.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_color(hex(if active {
                    pal.accent_text
                } else {
                    pal.text_secondary
                }))
                .child(glyph_icon(code, 15.)),
        )
        .when(alpha > 0.02, |el| {
            el.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .whitespace_nowrap()
                    .opacity(alpha)
                    .text_size(px(13.5))
                    .font_weight(if active {
                        gpui::FontWeight::SEMIBOLD
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(hex(if active { pal.text } else { pal.text_secondary }))
                    .child(label),
            )
            .when_some(count.filter(|c| *c > 0), |el, n| {
                el.child(
                    div()
                        .pr(px(10.))
                        .opacity(alpha)
                        .text_size(px(font::MICRO))
                        .text_color(hex(pal.text_muted))
                        .child(n.to_string()),
                )
            })
        })
        .tooltip(move |_, cx| make_tooltip(label, None, cx))
}

type NavAnchorSlot = Rc<Cell<Option<(f32, f32, bool)>>>;
type ScrollVpSlot = Rc<Cell<Option<(f32, f32)>>>;

/// Invisible zero-hit layout probe attached to whichever sidebar item is active.
/// Records its exact window-space `(top, height, in_scroll_area)` during `prepaint`
/// so the single unified accent pill glides seamlessly across every sidebar section.
fn nav_pill_anchor(slot: NavAnchorSlot, in_scroll: bool) -> impl IntoElement {
    canvas(
        move |bounds, _, _| {
            slot.set(Some((
                f32::from(bounds.origin.y),
                f32::from(bounds.size.height),
                in_scroll,
            )));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

pub struct SidebarRender {
    pub width: f32,
    pub alpha: f32,
    pub expanded: bool,
    pub content_offset: f32,
    pub is_overlay: bool,
    pub now: Instant,
}

impl VaultPreview {
    /// Stable identifier of the currently active sidebar item across all sections
    /// (Vault, Folders at any depth, Tools, and Bottom pinned items).
    fn active_nav_key(&self) -> String {
        match self.page {
            Page::Vault => {
                if let Some(folder_id) = &self.client.selected_folder {
                    format!("folder:{folder_id}")
                } else {
                    match self.filter {
                        Filter::All | Filter::Kind(_) => "all".into(),
                        Filter::Favorites => "favorites".into(),
                        Filter::Archived => "archived".into(),
                        Filter::Trash => "trash".into(),
                    }
                }
            }
            Page::Sends => "sends".into(),
            Page::Folders => "folders-page".into(),
            Page::Generator => "generator".into(),
            Page::Backup => "data".into(),
            Page::Settings => {
                if self.ui.settings_tab == SettingsTab::Data {
                    "data".into()
                } else {
                    "settings".into()
                }
            }
            Page::About => "settings".into(),
        }
    }

    pub fn render_sidebar(&self, frame: SidebarRender, cx: &mut Context<Self>) -> Stateful<Div> {
        let SidebarRender {
            width,
            alpha,
            expanded,
            content_offset,
            is_overlay,
            now,
        } = frame;
        let pal = theme::palette();
        let (total_active, total_fav, total_archived, total_trash) = self.ui.sidebar_counts(
            self.client.overview_generation,
            self.client.overview.as_ref(),
        );

        let anchor_slot: NavAnchorSlot = Rc::new(Cell::new(None));
        let scroll_vp_slot: ScrollVpSlot = Rc::new(Cell::new(None));

        let is_all = self.page == Page::Vault
            && self.client.selected_folder.is_none()
            && matches!(self.filter, Filter::All | Filter::Kind(_));
        let is_fav = self.page == Page::Vault
            && self.client.selected_folder.is_none()
            && self.filter == Filter::Favorites;
        let is_sends = self.page == Page::Sends;
        let is_generator = self.page == Page::Generator;
        let is_archived = self.page == Page::Vault
            && self.client.selected_folder.is_none()
            && self.filter == Filter::Archived;
        let is_trash = self.page == Page::Vault
            && self.client.selected_folder.is_none()
            && self.filter == Filter::Trash;
        let is_data_nav = self.page == Page::Backup
            || (self.page == Page::Settings && self.ui.settings_tab == SettingsTab::Data);
        let is_settings_nav = (self.page == Page::Settings
            && self.ui.settings_tab != SettingsTab::Data)
            || self.page == Page::About;

        let folder_tree = self.render_sidebar_folders(alpha, now, anchor_slot.clone(), cx);
        let active_key = self.active_nav_key();
        let tracker = if is_overlay {
            self.ui.nav_pill_overlay.clone()
        } else {
            self.ui.nav_pill_main.clone()
        };
        let animate = self.animate;
        let accent_fill = pal.accent_fill;
        let anchor_for_pill = anchor_slot.clone();
        let vp_for_pill = scroll_vp_slot.clone();
        let vp_for_probe = scroll_vp_slot.clone();

        div()
            .id("navigation")
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .min_h_0()
            .h_full()
            .w(px(width))
            .px(px(5.))
            .py(px(space::S))
            .gap(px(6.))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("toggle-sidebar")
                            .cursor_pointer()
                            .size(px(38.))
                            .rounded(px(radius::CONTROL))
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_shrink_0()
                            .text_color(hex(pal.text_secondary))
                            .hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
                            .active(move |s| s.bg(hex(pal.selected)))
                            .child(glyph_icon(glyph::MENU, 15.))
                            .tooltip(move |_, cx| {
                                make_tooltip(
                                    if expanded {
                                        "收起侧栏"
                                    } else {
                                        "展开侧栏"
                                    },
                                    Some("Ctrl+B"),
                                    cx,
                                )
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx)),
                            ),
                    )
                    .when(
                        alpha > 0.3 && self.write_available() && !self.selection.active,
                        |el| {
                            el.child(
                                btn("sidebar-new-item", BtnVariant::Tinted)
                                    .h(px(28.))
                                    .px(px(10.))
                                    .opacity(alpha)
                                    .text_size(px(font::CAPTION))
                                    .child(glyph_icon(glyph::ADD, 12.))
                                    .child("新增")
                                    .tooltip(|_, cx| make_tooltip("新增条目", Some("Ctrl+N"), cx))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_page(Page::Vault, cx);
                                        this.new_item(window, cx);
                                    })),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("navigation-items")
                            .relative()
                            .left(px(content_offset))
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .overflow_x_hidden()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            // Section 1: Vault core.
                            .when(alpha > 0.25, |el| {
                                el.child(
                                    div()
                                        .px(px(10.))
                                        .pt(px(4.))
                                        .pb(px(2.))
                                        .opacity(alpha)
                                        .text_size(px(font::MICRO))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(hex(pal.text_muted))
                                        .child("保险库"),
                                )
                            })
                            .child(
                                div()
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        sidebar_item(
                                            "nav-all",
                                            is_all,
                                            "全部条目",
                                            glyph::HOME,
                                            alpha,
                                            Some(total_active),
                                        )
                                        .when(is_all, |el| {
                                            el.child(nav_pill_anchor(anchor_slot.clone(), true))
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.select_vault_view(Filter::All, None, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        sidebar_item(
                                            "nav-favorites",
                                            is_fav,
                                            "收藏夹",
                                            glyph::STAR,
                                            alpha,
                                            Some(total_fav),
                                        )
                                        .when(is_fav, |el| {
                                            el.child(nav_pill_anchor(anchor_slot.clone(), true))
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.select_vault_view(Filter::Favorites, None, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        sidebar_item(
                                            "nav-sends",
                                            is_sends,
                                            "Send",
                                            glyph::SEND,
                                            alpha,
                                            None,
                                        )
                                        .when(is_sends, |el| {
                                            el.child(nav_pill_anchor(anchor_slot.clone(), true))
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.open_page(Page::Sends, cx);
                                            }),
                                        ),
                                    ),
                            )
                            // Section 2: Folders (inline collapsible tree when expanded, icon button when rail).
                            .child(folder_tree)
                            // Section 3: Tools.
                            .when(alpha > 0.25, |el| {
                                el.child(
                                    div()
                                        .px(px(10.))
                                        .pt(px(8.))
                                        .pb(px(2.))
                                        .opacity(alpha)
                                        .text_size(px(font::MICRO))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(hex(pal.text_muted))
                                        .child("工具"),
                                )
                            })
                            .child(
                                div()
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        sidebar_item(
                                            "nav-generator",
                                            is_generator,
                                            "密码生成器",
                                            glyph::KEY,
                                            alpha,
                                            None,
                                        )
                                        .when(is_generator, |el| {
                                            el.child(nav_pill_anchor(anchor_slot.clone(), true))
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.open_page(Page::Generator, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        sidebar_item(
                                            "nav-backup",
                                            is_data_nav,
                                            "导入与导出",
                                            glyph::EXPORT,
                                            alpha,
                                            None,
                                        )
                                        .when(is_data_nav, |el| {
                                            el.child(nav_pill_anchor(anchor_slot.clone(), true))
                                        })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                let prev_tab = this.ui.settings_tab.index();
                                                this.ui.animate_segmented(
                                                    ElementId::Name("settings-tabs".into()),
                                                    prev_tab,
                                                    SettingsTab::Data.index(),
                                                    this.animate,
                                                );
                                                this.ui.settings_tab = SettingsTab::Data;
                                                this.open_page(Page::Settings, cx);
                                            }),
                                        ),
                                    ),
                            ),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                let top = f32::from(bounds.origin.y);
                                let bottom = top + f32::from(bounds.size.height);
                                vp_for_probe.set(Some((top, bottom)));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    ),
            )
            // Bottom pinned section: Archive, Trash, Settings.
            .child(
                div()
                    .relative()
                    .left(px(content_offset))
                    .flex_shrink_0()
                    .pt(px(4.))
                    .border_t_1()
                    .border_color(hex(pal.border_subtle))
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                sidebar_item(
                                    "nav-archived",
                                    is_archived,
                                    "归档",
                                    glyph::ARCHIVE,
                                    alpha,
                                    Some(total_archived),
                                )
                                .when(is_archived, |el| {
                                    el.child(nav_pill_anchor(anchor_slot.clone(), false))
                                })
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.select_vault_view(Filter::Archived, None, cx);
                                    },
                                )),
                            )
                            .child(
                                sidebar_item(
                                    "nav-trash",
                                    is_trash,
                                    "回收站",
                                    glyph::DELETE,
                                    alpha,
                                    Some(total_trash),
                                )
                                .when(is_trash, |el| {
                                    el.child(nav_pill_anchor(anchor_slot.clone(), false))
                                })
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.select_vault_view(Filter::Trash, None, cx);
                                    },
                                )),
                            )
                            .child(
                                sidebar_item(
                                    "nav-settings",
                                    is_settings_nav,
                                    "设置",
                                    glyph::SETTINGS,
                                    alpha,
                                    None,
                                )
                                .when(is_settings_nav, |el| {
                                    el.child(nav_pill_anchor(anchor_slot, false))
                                })
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if this.ui.settings_tab == SettingsTab::Data {
                                            let prev = this.ui.settings_tab.index();
                                            this.ui.settings_tab = SettingsTab::Appearance;
                                            this.ui.animate_segmented(
                                                ElementId::Name("settings-tabs".into()),
                                                prev,
                                                SettingsTab::Appearance.index(),
                                                this.animate,
                                            );
                                        }
                                        this.open_page(Page::Settings, cx);
                                    },
                                )),
                            ),
                    ),
            )
            // Unified cross-section elastic gliding accent pill overlay (prepainted last
            // so all child anchors have already recorded their exact frame coordinates).
            .child(
                canvas(
                    move |sidebar_bounds, window, _| -> Option<(f32, f32)> {
                        let (item_top, item_h, in_scroll) = anchor_for_pill.get()?;
                        let sidebar_top = f32::from(sidebar_bounds.origin.y);
                        let target_y = (item_top - sidebar_top) + (item_h - 18.) * 0.5;
                        let mut state = tracker.borrow_mut();
                        if !state.initialized {
                            state.initialized = true;
                            state.active_key = active_key.clone();
                            state.spring.snap(target_y, now);
                        } else if state.active_key != active_key {
                            state.active_key = active_key.clone();
                            state.spring.retarget(target_y, now, animate);
                        } else if (state.spring.target() - target_y).abs() > 0.25 {
                            if state.spring.is_settled() {
                                state.spring.snap(target_y, now);
                            } else {
                                state.spring.retarget(target_y, now, animate);
                            }
                        }
                        let (cur_y, cur_vel, moving) =
                            state.spring.sample_with_velocity(now, animate);
                        if moving {
                            window.request_animation_frame();
                        }
                        if in_scroll && !moving {
                            let pill_center = sidebar_top + cur_y + 9.;
                            if let Some((vp_top, vp_bot)) = vp_for_pill.get()
                                && (pill_center < vp_top + 2. || pill_center > vp_bot - 2.)
                            {
                                return None;
                            }
                        }
                        let stretch = if animate {
                            (cur_vel.abs() * 0.014).min(18.)
                        } else {
                            0.
                        };
                        Some((cur_y, stretch))
                    },
                    move |sidebar_bounds, geom, window, _| {
                        let Some((cur_y, stretch)) = geom else {
                            return;
                        };
                        let pill_h = 18. + stretch;
                        let x = sidebar_bounds.origin.x + px(5. + content_offset + 3.);
                        let y = sidebar_bounds.origin.y + px(cur_y - stretch * 0.5);
                        let mut quad = fill(
                            Bounds::new(point(x, y), size(px(3.), px(pill_h))),
                            hex(accent_fill),
                        );
                        quad.corner_radii = Corners::all(px(1.5));
                        window.paint_quad(quad);
                    },
                )
                .absolute()
                .inset_0(),
            )
    }

    fn render_sidebar_folders(
        &self,
        alpha: f32,
        now: Instant,
        anchor_slot: NavAnchorSlot,
        cx: &mut Context<Self>,
    ) -> Div {
        let pal = theme::palette();
        // When collapsed to rail, show a single clean Folders icon button.
        if alpha <= 0.25 {
            let active = self.page == Page::Folders || self.client.selected_folder.is_some();
            return div().child(
                sidebar_item("folders-page", active, "文件夹", glyph::FOLDER, alpha, None)
                    .when(active, |el| el.child(nav_pill_anchor(anchor_slot, true)))
                    .on_click(cx.listener(|this, _, _, cx| this.open_page(Page::Folders, cx))),
            );
        }

        let mut section = div().flex().flex_col().gap(px(2.)).pt(px(6.)).child(
            div()
                .px(px(10.))
                .py(px(2.))
                .opacity(alpha)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .id("folders-header-manage")
                        .cursor_pointer()
                        .text_size(px(font::MICRO))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(hex(pal.text_muted))
                        .hover(move |s| s.text_color(hex(pal.text)))
                        .child("文件夹")
                        .tooltip(|_, cx| make_tooltip("管理全部文件夹", None, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.open_page(Page::Folders, cx))),
                )
                .child(
                    div()
                        .id("folders-manage-btn")
                        .cursor_pointer()
                        .size(px(22.))
                        .rounded(px(radius::SM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(hex(pal.text_muted))
                        .hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
                        .child(glyph_icon(glyph::ADD, 11.))
                        .tooltip(|_, cx| make_tooltip("管理或新建文件夹", None, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.open_page(Page::Folders, cx))),
                ),
        );

        let Some(overview) = self.client.overview.as_ref() else {
            let active = self.page == Page::Folders;
            return section.child(
                sidebar_item(
                    "folders-page",
                    active,
                    "我的文件夹",
                    glyph::FOLDER,
                    alpha,
                    None,
                )
                .when(active, |el| el.child(nav_pill_anchor(anchor_slot, true)))
                .on_click(cx.listener(|this, _, _, cx| this.open_page(Page::Folders, cx))),
            );
        };

        let roots = self.folders.root_indices();
        if roots.is_empty() {
            let active = self.page == Page::Folders;
            return section.child(
                sidebar_item(
                    "folders-page",
                    active,
                    "我的文件夹",
                    glyph::FOLDER,
                    alpha,
                    None,
                )
                .when(active, |el| el.child(nav_pill_anchor(anchor_slot, true)))
                .on_click(cx.listener(|this, _, _, cx| this.open_page(Page::Folders, cx))),
            );
        }

        // Flatten up to 24 visible tree nodes for snappy rendering (for 1000+ folders, link to Folders page).
        let mut visible_nodes: Vec<(usize, usize)> = Vec::new();
        self.collect_folder_tree_nodes(roots, 0, &mut visible_nodes, 24);

        for (i, (row_index, depth)) in visible_nodes.into_iter().enumerate() {
            let Some((folder_id, label)) = self.folders.sidebar_row(row_index, overview) else {
                continue;
            };
            let folder_id_str = folder_id.to_owned();
            let toggle_id = folder_id_str.clone();
            let select_id = folder_id_str.clone();
            let has_children = !self.folders.child_indices(row_index).is_empty();
            let is_expanded = self.ui.expanded_folders.contains(&folder_id_str);
            let chevron_key = ElementId::Name(format!("folder-tree-{folder_id_str}").into());
            let chevron_p =
                self.ui
                    .collapsible_progress(&chevron_key, is_expanded, now, self.animate);
            let is_active = self.page == Page::Vault
                && self.client.selected_folder.as_deref() == Some(folder_id);
            let indent = (depth.min(3) as f32) * 14.;
            let count =
                self.ui
                    .folder_item_count(self.client.overview_generation, overview, folder_id);

            section = section.child(
                div()
                    .id(("sidebar-folder", i))
                    .cursor_pointer()
                    .relative()
                    .h(px(32.))
                    .pl(px(8. + indent))
                    .pr(px(8.))
                    .rounded(px(radius::CONTROL))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .opacity(alpha)
                    .when(is_active, |el| {
                        el.bg(hex(pal.selected))
                            .child(nav_pill_anchor(anchor_slot.clone(), true))
                    })
                    .when(!is_active, |el| el.hover(move |s| s.bg(hex(pal.hover))))
                    .child(if has_children {
                        div()
                            .id(("sidebar-folder-toggle", i))
                            .cursor_pointer()
                            .size(px(18.))
                            .rounded(px(radius::SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(hex(pal.text_muted))
                            .hover(move |s| s.bg(hex(pal.selected)).text_color(hex(pal.text)))
                            .child(rotating_chevron(chevron_p, 12., hex(pal.text_muted).into()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let next_open = !this.ui.expanded_folders.remove(&toggle_id);
                                if next_open {
                                    this.ui.expanded_folders.insert(toggle_id.clone());
                                }
                                this.ui.animate_collapsible(
                                    ElementId::Name(format!("folder-tree-{toggle_id}").into()),
                                    next_open,
                                    this.animate,
                                );
                                cx.notify();
                            }))
                            .into_any_element()
                    } else {
                        div().w(px(18.)).flex_shrink_0().into_any_element()
                    })
                    .child(
                        div()
                            .text_color(hex(if is_active {
                                pal.accent_text
                            } else {
                                pal.text_secondary
                            }))
                            .child(glyph_icon(
                                if is_active || is_expanded {
                                    glyph::FOLDER_OPEN
                                } else {
                                    glyph::FOLDER
                                },
                                13.,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(if is_active {
                                gpui::FontWeight::SEMIBOLD
                            } else {
                                gpui::FontWeight::NORMAL
                            })
                            .text_color(hex(if is_active {
                                pal.text
                            } else {
                                pal.text_secondary
                            }))
                            .child(label.to_owned()),
                    )
                    .when(count > 0, |el| {
                        el.child(
                            div()
                                .text_size(px(font::MICRO))
                                .text_color(hex(pal.text_muted))
                                .child(count.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_vault_view(Filter::All, Some(select_id.clone()), cx);
                    })),
            );
        }

        if self.folders.total_count() > 24 || self.page == Page::Folders {
            let active = self.page == Page::Folders;
            section = section.child(
                div()
                    .id("folders-page")
                    .cursor_pointer()
                    .relative()
                    .h(px(30.))
                    .px(px(12.))
                    .rounded(px(radius::CONTROL))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .opacity(alpha)
                    .text_size(px(font::CAPTION))
                    .font_weight(if active {
                        gpui::FontWeight::SEMIBOLD
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(hex(pal.accent_text))
                    .when(active, |el| {
                        el.bg(hex(pal.selected))
                            .child(nav_pill_anchor(anchor_slot, true))
                    })
                    .when(!active, |el| el.hover(move |s| s.bg(hex(pal.hover))))
                    .child(format!("管理全部文件夹（{}）", self.folders.total_count()))
                    .on_click(cx.listener(|this, _, _, cx| this.open_page(Page::Folders, cx))),
            );
        }
        section
    }

    fn collect_folder_tree_nodes(
        &self,
        indices: &[usize],
        depth: usize,
        out: &mut Vec<(usize, usize)>,
        limit: usize,
    ) {
        let Some(overview) = self.client.overview.as_ref() else {
            return;
        };
        for &row_index in indices {
            if out.len() >= limit {
                return;
            }
            out.push((row_index, depth));
            if let Some((id, _)) = self.folders.sidebar_row(row_index, overview)
                && self.ui.expanded_folders.contains(id)
            {
                let children = self.folders.child_indices(row_index);
                self.collect_folder_tree_nodes(children, depth + 1, out, limit);
            }
        }
    }

    pub fn select_vault_view(
        &mut self,
        filter: Filter,
        folder: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.client.form.is_some()
            || self.client.editor.is_some()
            || self.lock_queued
            || self.selection.pending.is_some()
        {
            return;
        }
        self.sidebar.dismiss();
        let changed = self.page != Page::Vault
            || self.filter != filter
            || self.client.selected_folder != folder;
        if self.page != Page::Vault {
            self.open_page(Page::Vault, cx);
        }
        if changed {
            self.selection.clear();
            self.filter = filter;
            self.client.selected_folder = folder;
            self.invalidate_query();
            self.ui.trigger_list_motion(self.animate);
        }
        cx.notify();
    }

    /// Floating spring-animated Account Switcher Popover anchored below the titlebar Avatar.
    pub fn render_account_popover(
        &self,
        progress: f32,
        compact_chrome: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let pal = theme::palette();
        let opacity = clamp_opacity(progress);
        let y_offset = 12. * (1. - progress);
        let anchor_right = if compact_chrome { 12. } else { 142. };

        div()
            .id("account-popover-backdrop")
            .absolute()
            .top(px(super::tokens::size::TITLEBAR))
            .bottom_0()
            .left_0()
            .right_0()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.client.show_accounts = false;
                    cx.notify();
                }),
            )
            .child(
                elevated_card()
                    .absolute()
                    .top(px(6. - y_offset))
                    .right(px(anchor_right))
                    .w(px(320.))
                    .max_h(px(380.))
                    .opacity(opacity)
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px(px(14.))
                            .py(px(10.))
                            .border_b_1()
                            .border_color(hex(pal.border_subtle))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(hex(pal.text_secondary))
                                    .child(format!(
                                        "已连接账户（{}/5）",
                                        self.client.accounts.len()
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id("accounts-popover-list")
                            .overflow_y_scroll()
                            .p(px(6.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .children(self.client.accounts.iter().enumerate().map(
                                |(i, account)| {
                                    let id = account.id.clone();
                                    let (server_tag, server_name) = match account.server_mode {
                                        0 => ("US", "Bitwarden.com"),
                                        1 => ("EU", "Bitwarden.eu"),
                                        _ => ("自", account.server_url.as_str()),
                                    };
                                    div()
                                        .id(("account", i))
                                        .cursor_pointer()
                                        .px(px(8.))
                                        .py(px(8.))
                                        .rounded(px(radius::CONTROL))
                                        .flex()
                                        .items_center()
                                        .gap(px(10.))
                                        .when(account.active, |row| {
                                            row.bg(pal.tint(pal.accent_fill))
                                                .border_1()
                                                .border_color(pal.tint_border(pal.accent_fill))
                                        })
                                        .when(!account.active, |row| {
                                            row.hover(move |s| s.bg(hex(pal.hover)))
                                        })
                                        .child(avatar(&account.email, Some(server_tag), 30.))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_size(px(13.))
                                                        .font_weight(gpui::FontWeight::MEDIUM)
                                                        .text_color(hex(pal.text))
                                                        .child(account.email.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_size(px(font::MICRO))
                                                        .text_color(hex(pal.text_muted))
                                                        .child(server_name.to_owned()),
                                                ),
                                        )
                                        .when(account.active, |row| {
                                            row.child(
                                                div()
                                                    .text_color(hex(pal.accent_text))
                                                    .child(glyph_icon(glyph::CHECK, 12.)),
                                            )
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_account(id.clone(), cx)
                                        }))
                                },
                            )),
                    )
                    .child(
                        div()
                            .p(px(8.))
                            .border_t_1()
                            .border_color(hex(pal.border_subtle))
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .child(
                                btn("add-account", BtnVariant::Secondary)
                                    .flex_1()
                                    .h(px(30.))
                                    .text_size(px(font::CAPTION))
                                    .child(glyph_icon(glyph::ADD, 12.))
                                    .child("添加账户")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.begin_sign_in(window, cx)
                                    })),
                            )
                            .when(
                                self.client
                                    .active()
                                    .is_some_and(|account| account.needs_sign_in),
                                |el| {
                                    el.child(
                                        btn("reauthenticate", BtnVariant::Primary)
                                            .h(px(30.))
                                            .text_size(px(font::CAPTION))
                                            .child("重新登录")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.reauthenticate(window, cx)
                                            })),
                                    )
                                },
                            )
                            .when(self.client.active().is_some(), |el| {
                                el.child(
                                    btn("logout", BtnVariant::Danger)
                                        .h(px(30.))
                                        .text_size(px(font::CAPTION))
                                        .child("退出登录")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.request_logout(cx)),
                                        ),
                                )
                            }),
                    ),
            )
    }

    pub fn toggle_shortcuts_modal(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.ui.shortcuts_open = !self.ui.shortcuts_open;
        self.ui.shortcuts_spring.retarget(
            if self.ui.shortcuts_open { 1. } else { 0. },
            now,
            self.animate,
        );
        cx.notify();
    }

    pub fn render_shortcuts_modal(&self, progress: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let pal = theme::palette();
        let opacity = clamp_opacity(progress);
        let y_offset = 20. * (1. - progress);

        let groups: [(&str, &[(&str, &str)]); 3] = [
            (
                "全局导航与命令",
                &[
                    ("打开命令面板", "Ctrl+K"),
                    ("聚焦搜索框", "Ctrl+F"),
                    ("展开 / 收起侧栏", "Ctrl+B"),
                    ("同步保险库", "Ctrl+R"),
                    ("立即锁定保险库", "Ctrl+L"),
                    ("快捷键指南", "Ctrl+/"),
                ],
            ),
            (
                "条目与快速复制",
                &[
                    ("新建条目", "Ctrl+N"),
                    ("编辑当前条目", "Ctrl+E"),
                    ("保存编辑", "Ctrl+S"),
                    ("复制用户名", "Ctrl+Shift+U"),
                    ("复制密码", "Ctrl+Shift+P"),
                    ("复制验证码 (TOTP)", "Ctrl+Shift+T"),
                ],
            ),
            (
                "批量选择",
                &[
                    ("进入 / 退出批量模式", "Ctrl+Shift+S"),
                    ("全选当前筛选结果", "Ctrl+Shift+A"),
                    ("清除已选条目", "Ctrl+Shift+D"),
                    ("关闭弹窗或取消", "Esc"),
                ],
            ),
        ];

        div()
            .id("shortcuts-backdrop")
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(pal.scrim(opacity))
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p(px(space::XL))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.toggle_shortcuts_modal(cx)),
            )
            .child(
                elevated_card()
                    .relative()
                    .top(px(y_offset))
                    .opacity(opacity)
                    .w_full()
                    .max_w(px(520.))
                    .max_h(px(540.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px(px(space::L))
                            .py(px(space::M))
                            .border_b_1()
                            .border_color(hex(pal.border_subtle))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .text_color(hex(pal.accent_text))
                                            .child(glyph_icon(glyph::KEYBOARD, 16.)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(font::BODY_LG))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(hex(pal.text))
                                            .child("键盘快捷键"),
                                    ),
                            )
                            .child(
                                icon_btn("close-shortcuts", glyph::CLOSE, "关闭", Some("Esc"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_shortcuts_modal(cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("shortcuts-body")
                            .overflow_y_scroll()
                            .p(px(space::L))
                            .flex()
                            .flex_col()
                            .gap(px(space::L))
                            .children(groups.into_iter().map(|(section, items)| {
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_size(px(font::CAPTION))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(hex(pal.text_secondary))
                                            .child(section),
                                    )
                                    .children(items.iter().map(|(label, keys)| {
                                        div()
                                            .py(px(5.))
                                            .px(px(8.))
                                            .rounded(px(radius::CONTROL))
                                            .bg(hex(pal.layer))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_size(px(13.))
                                                    .text_color(hex(pal.text))
                                                    .child(*label),
                                            )
                                            .child(
                                                div()
                                                    .px(px(7.))
                                                    .py(px(2.))
                                                    .rounded(px(radius::SM))
                                                    .bg(hex(pal.card))
                                                    .border_1()
                                                    .border_color(hex(pal.border))
                                                    .text_size(px(font::MICRO))
                                                    .font_family(family::MONO)
                                                    .text_color(hex(pal.text_secondary))
                                                    .child(*keys),
                                            )
                                    }))
                            })),
                    ),
            )
    }

    /// Floating animated Modal wrapper for in-vault prompts (reprompt, delete confirmation, bulk confirmation).
    pub fn render_modal_overlay(
        &self,
        progress: f32,
        content: Div,
        on_backdrop_close: impl Fn(&mut Self, &mut gpui::Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let pal = theme::palette();
        let opacity = clamp_opacity(progress);
        let y_offset = 22. * (1. - progress);

        div()
            .id("modal-overlay-backdrop")
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0()
            .bg(pal.scrim(opacity))
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p(px(space::XL))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| on_backdrop_close(this, window, cx)),
            )
            .child(
                div()
                    .relative()
                    .top(px(y_offset))
                    .opacity(opacity)
                    .w_full()
                    .max_w(px(460.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(content),
            )
    }

    /// Render a non-sensitive visual ghost card while a dismissed modal finishes its spring exit.
    pub fn render_closing_modal_card(snapshot: &ModalClosingSnapshot) -> Div {
        let pal = theme::palette();
        elevated_card()
            .flex()
            .flex_col()
            .gap(px(space::M))
            .p(px(20.))
            .max_w(px(460.))
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .size(px(34.))
                            .rounded(px(radius::PILL))
                            .bg(pal.tint(if snapshot.danger {
                                pal.danger
                            } else {
                                pal.accent_fill
                            }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(hex(if snapshot.danger {
                                pal.danger
                            } else {
                                pal.accent_text
                            }))
                            .child(glyph_icon(
                                if snapshot.danger {
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
                                    .child(snapshot.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child(snapshot.subtitle.clone()),
                            ),
                    ),
            )
            .child(
                div()
                    .pt(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .h(px(36.))
                            .rounded(px(radius::CONTROL))
                            .bg(if snapshot.danger {
                                pal.tint(pal.danger)
                            } else {
                                hex(pal.accent_fill).into()
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(font::BODY))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(hex(if snapshot.danger {
                                pal.danger
                            } else {
                                pal.on_accent
                            }))
                            .child(snapshot.action_label),
                    )
                    .child(
                        div()
                            .h(px(36.))
                            .px(px(16.))
                            .rounded(px(radius::CONTROL))
                            .bg(hex(pal.card))
                            .border_1()
                            .border_color(hex(pal.border))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(font::BODY))
                            .text_color(hex(pal.text))
                            .child("返回"),
                    ),
            )
    }

    /// Render persistent status badge / InfoBar only when actually needed (offline, error, warning, pending cancel).
    pub fn render_persistent_banner(&self, now: Instant, cx: &Context<Self>) -> Option<Div> {
        let pal = theme::palette();
        let (banner_p, _) =
            self.ui
                .banner_spring
                .sample_staggered(now, Duration::ZERO, self.animate);
        let wrap = |bar: Div| {
            div()
                .relative()
                .top(px(-6. * (1. - banner_p)))
                .opacity(clamp_opacity(banner_p))
                .child(bar)
        };
        if self.client.pending.is_some() {
            let label = self
                .client
                .busy
                .map_or_else(|| self.client.notice.clone(), |b| format!("{b}…"));
            return Some(wrap(
                super::components::info_bar(super::toast::ToastKind::Info, label).child(
                    btn("cancel-operation", BtnVariant::Secondary)
                        .h(px(26.))
                        .px(px(10.))
                        .text_size(px(font::CAPTION))
                        .child("取消请求")
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_pending(cx))),
                ),
            ));
        }
        let is_read_only = self
            .client
            .overview
            .as_ref()
            .is_some_and(|overview| overview.read_only);
        let on_gate_screen = self.page == Page::Vault
            && self.client.overview.is_none()
            && self.client.form.is_none();
        let notice = self.client.notice.trim();
        let (kind, persistent) = super::state::UiState::classify_notice(notice);
        if persistent && !notice.is_empty() {
            if on_gate_screen && !matches!(kind, super::toast::ToastKind::Error) {
                return None;
            }
            return Some(wrap(
                super::components::info_bar(kind, notice.to_owned())
                    .when(
                        is_read_only
                            && (self.client.sync_available() || self.client.reconnect_available()),
                        |el| {
                            el.child(
                                btn("banner-sync-now", BtnVariant::Primary)
                                    .h(px(26.))
                                    .px(px(10.))
                                    .text_size(px(font::CAPTION))
                                    .child(if self.client.reconnect_available() {
                                        "重新连接"
                                    } else {
                                        "立即同步"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.sync_or_reconnect(window, cx)
                                    })),
                            )
                        },
                    )
                    .child(
                        div()
                            .id("dismiss-banner")
                            .cursor_pointer()
                            .size(px(22.))
                            .rounded(px(radius::SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(hex(pal.text_muted))
                            .hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
                            .child(glyph_icon(glyph::CLOSE, 10.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.client.notice.clear();
                                cx.notify();
                            })),
                    ),
            ));
        }
        if is_read_only && self.page == Page::Vault {
            return Some(wrap(
                super::components::info_bar(
                    super::toast::ToastKind::Warning,
                    "当前处于离线只读快照模式，同步成功后可编辑。",
                )
                .when(
                    self.client.sync_available() || self.client.reconnect_available(),
                    |el| {
                        el.child(
                            btn("offline-sync-now", BtnVariant::Primary)
                                .h(px(26.))
                                .px(px(10.))
                                .text_size(px(font::CAPTION))
                                .child(if self.client.reconnect_available() {
                                    "重新连接"
                                } else {
                                    "立即同步"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.sync_or_reconnect(window, cx)
                                })),
                        )
                    },
                ),
            ));
        }
        None
    }

    pub fn render_server_badge(server_mode: u32, server_url: &str) -> Div {
        let pal = theme::palette();
        match server_mode {
            0 => badge("US 云端", pal.info),
            1 => badge("EU 云端", pal.info),
            _ => badge(
                if server_url.is_empty() {
                    "自托管".to_owned()
                } else {
                    format!("自托管 · {server_url}")
                },
                pal.accent_text,
            ),
        }
    }
}
