use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    time::{Duration, Instant},
};

use bytewarden_core::desktop::{Folder, Overview, Request};
use gpui::{
    Context, ElementId, Entity, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use crate::{Page, VaultPreview, input::Input};

#[derive(Default)]
pub struct Folders {
    pub selected: Option<String>,
    selected_ids: HashSet<String>,
    form: Option<FolderForm>,
    generation: Option<u64>,
    rows: Rc<[FolderRow]>,
    by_id: HashMap<String, usize>,
    scroll: UniformListScrollHandle,
    picker_scroll: UniformListScrollHandle,
    editor_scroll: UniformListScrollHandle,
    search: Option<Entity<Input>>,
    query: String,
    search_keys: Vec<String>,
    roots: Rc<[usize]>,
    children: Vec<Rc<[usize]>>,
    visible: Rc<[usize]>,
    manager: FolderNavigation,
    vault: FolderNavigation,
}

struct FolderRow {
    source: usize,
    depth: usize,
    parent: Option<usize>,
    display_start: usize,
    full_name: gpui::SharedString,
    display: gpui::SharedString,
    full_display: gpui::SharedString,
}

#[derive(Default)]
struct FolderNavigation {
    id: Option<String>,
    trail: Rc<[usize]>,
    earlier_open: bool,
    scroll: UniformListScrollHandle,
    children_scroll: UniformListScrollHandle,
}

impl FolderNavigation {
    fn select(&mut self, id: Option<&str>, by_id: &HashMap<String, usize>, rows: &[FolderRow]) {
        let id = id.filter(|id| by_id.contains_key(*id));
        if self.id.as_deref() == id {
            return;
        }
        self.id = id.map(str::to_owned);
        let mut trail = Vec::with_capacity(
            id.and_then(|id| by_id.get(id))
                .map_or(0, |index| rows[*index].depth + 1),
        );
        let mut next = id.and_then(|id| by_id.get(id).copied());
        while let Some(index) = next {
            trail.push(index);
            next = rows[index].parent;
        }
        trail.reverse();
        self.trail = trail.into();
        self.earlier_open = false;
        self.scroll = UniformListScrollHandle::new();
        self.children_scroll = UniformListScrollHandle::new();
    }
}

impl Folders {
    pub fn refresh(&mut self, generation: u64, overview: Option<&Overview>) {
        if self.generation == Some(generation) {
            return;
        }
        self.rows =
            overview.map_or_else(|| Rc::from([]), |overview| labels(&overview.folders).into());
        self.by_id = overview.map_or_else(HashMap::new, |overview| {
            self.rows
                .iter()
                .enumerate()
                .map(|(index, row)| (overview.folders[row.source].id.clone(), index))
                .collect()
        });
        self.selected_ids.retain(|id| self.by_id.contains_key(id));
        self.search_keys = overview.map_or_else(Vec::new, |overview| {
            self.rows
                .iter()
                .map(|row| query_key(&overview.folders[row.source].name))
                .collect()
        });
        let mut roots = Vec::new();
        let mut children = vec![Vec::new(); self.rows.len()];
        for (index, row) in self.rows.iter().enumerate() {
            if let Some(parent) = row.parent {
                children[parent].push(index);
            } else {
                roots.push(index);
            }
        }
        self.roots = roots.into();
        self.children = children.into_iter().map(Rc::from).collect();
        let manager_id = self.manager.id.take();
        self.manager = FolderNavigation::default();
        self.manager
            .select(manager_id.as_deref(), &self.by_id, &self.rows);
        self.vault = FolderNavigation::default();
        self.selected = self
            .selected
            .take()
            .filter(|id| self.by_id.contains_key(id));
        self.update_visible();
        self.scroll = UniformListScrollHandle::new();
        self.picker_scroll = UniformListScrollHandle::new();
        self.editor_scroll = UniformListScrollHandle::new();
        self.generation = Some(generation);
    }

    pub fn name<'a>(&self, id: &str, overview: Option<&'a Overview>) -> &'a str {
        self.by_id
            .get(id)
            .and_then(|index| overview?.folders.get(self.rows.get(*index)?.source))
            .map_or("所有文件夹", |folder| folder.name.as_str())
    }

    fn toggle_bulk_selection(&mut self, id: String) -> bool {
        if self.selected_ids.remove(&id) {
            return true;
        }
        if self.selected_ids.len() >= 500 {
            return false;
        }
        self.selected_ids.insert(id);
        true
    }

    pub(crate) fn root_indices(&self) -> &[usize] {
        &self.roots
    }

    pub(crate) fn child_indices(&self, row_index: usize) -> &[usize] {
        self.children
            .get(row_index)
            .map_or(&[], |children| children.as_ref())
    }

    pub(crate) fn sidebar_row<'a>(
        &'a self,
        row_index: usize,
        overview: &'a Overview,
    ) -> Option<(&'a str, &'a str)> {
        let row = self.rows.get(row_index)?;
        let folder = overview.folders.get(row.source)?;
        Some((folder.id.as_str(), row.display.as_ref()))
    }

    pub(crate) fn total_count(&self) -> usize {
        self.rows.len()
    }

    pub(crate) fn has_form(&self) -> bool {
        self.form.is_some()
    }

    pub(crate) fn dismiss_form(&mut self) {
        self.form = None;
    }

    pub(crate) fn form_snapshot(&self) -> Option<crate::ui::state::ModalClosingSnapshot> {
        let form = self.form.as_ref()?;
        let (title, danger, action_label) = match form.op {
            FolderOp::Create => ("新建文件夹", false, "保存"),
            FolderOp::Rename(_) => ("重命名文件夹", false, "保存"),
            FolderOp::Delete(_) => ("删除文件夹？", true, "确认删除"),
            FolderOp::DeleteMany(_) => ("删除所选文件夹？", true, "确认删除"),
        };
        Some(crate::ui::state::ModalClosingSnapshot {
            title: title.into(),
            subtitle: "修改不会级联，子文件夹及其中条目会保留。".into(),
            danger,
            action_label,
        })
    }

    fn set_query(&mut self, query: &str) -> bool {
        let query = query_key(query.trim());
        if self.query == query {
            return false;
        }
        self.query = query;
        self.selected = None;
        self.update_visible();
        self.scroll = UniformListScrollHandle::new();
        true
    }

    fn update_visible(&mut self) {
        self.visible = if self.query.is_empty() {
            self.manager
                .id
                .as_ref()
                .and_then(|id| self.by_id.get(id))
                .map_or_else(|| self.roots.clone(), |index| self.children[*index].clone())
        } else {
            self.search_keys
                .iter()
                .enumerate()
                .filter_map(|(index, key)| key.contains(&self.query).then_some(index))
                .collect::<Vec<_>>()
                .into()
        };
    }
}

// Use one Unicode scalar per case key: expansion (ß -> SS) must not make two
// distinct ordinal folder names match. Keys are built once per snapshot/query.
fn query_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            let mut upper = ch.to_uppercase();
            let first = upper.next().unwrap_or(ch);
            if upper.next().is_none() { first } else { ch }
        })
        .collect()
}

fn compact_name(name: &str) -> String {
    if name.trim().is_empty() {
        return "—".into();
    }
    let mut chars = name.chars();
    let head = chars.by_ref().take(64).collect::<String>();
    let tail = chars.collect::<Vec<_>>();
    if tail.len() <= 96 {
        return name.to_owned();
    }
    head + "…" + &tail[tail.len() - 94..].iter().collect::<String>()
}

#[derive(Clone)]
enum FolderOp {
    Create,
    Rename(String),
    Delete(String),
    DeleteMany(Vec<String>),
}
struct FolderForm {
    op: FolderOp,
    name: Entity<Input>,
    must_reconcile: bool,
}

impl FolderForm {
    fn record_error(&mut self, code: &str) {
        self.must_reconcile |= matches!(code, "conflict" | "notSynced" | "writeUncertain");
    }
}

fn labels(folders: &[Folder]) -> Vec<FolderRow> {
    // Component ordering keeps each existing subtree adjacent, including names
    // such as A!, A/B and missing ancestors. No synthetic parent rows are made.
    let mut ids = HashSet::new();
    let mut ordered = (0..folders.len())
        .filter(|index| !folders[*index].id.is_empty() && ids.insert(folders[*index].id.as_str()))
        .collect::<Vec<_>>();
    let path = |index: usize| folders[index].name.trim_matches('/');
    ordered.sort_unstable_by(|a, b| {
        path(*a)
            .split('/')
            .cmp(path(*b).split('/'))
            .then(folders[*a].id.cmp(&folders[*b].id))
    });
    let mut ancestors = Vec::<usize>::new();
    let mut rows = Vec::<FolderRow>::with_capacity(folders.len());
    let mut previous: Option<usize> = None;
    for source in ordered {
        let name = path(source);
        if let Some(previous) = previous.filter(|previous| path(rows[*previous].source) == name) {
            let row = &rows[previous];
            // Duplicate paths remain separate IDs. Only the first representative
            // owns children, matching WinUI's hierarchy without inventing IDs.
            rows.push(FolderRow {
                source,
                depth: row.depth,
                parent: row.parent,
                display_start: row.display_start,
                full_name: folders[source].name.clone().into(),
                display: row.display.clone(),
                full_display: compact_name(&folders[source].name).into(),
            });
            continue;
        }
        while let Some(parent) = ancestors.last() {
            let parent = path(rows[*parent].source);
            if !parent.is_empty()
                && name
                    .strip_prefix(parent)
                    .is_some_and(|tail| tail.starts_with('/'))
            {
                break;
            }
            ancestors.pop();
        }
        let parent = ancestors.last().copied();
        let display_start = parent.map_or(0, |parent| path(rows[parent].source).len() + 1);
        let index = rows.len();
        let display = &name[display_start..];
        rows.push(FolderRow {
            source,
            depth: ancestors.len(),
            parent,
            display_start,
            full_name: folders[source].name.clone().into(),
            display: compact_name(if display.trim().is_empty() {
                "—"
            } else {
                display
            })
            .into(),
            full_display: compact_name(&folders[source].name).into(),
        });
        ancestors.push(index);
        previous = Some(index);
    }
    rows
}

impl VaultPreview {
    pub(crate) fn render_folder_picker(
        &self,
        for_editor: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{elevated_card, glyph_icon, make_tooltip},
            tokens::{font, glyph, radius},
        };
        let special = if for_editor { 1 } else { 2 };
        let count = self.folders.rows.len() + special;
        let scroll = if for_editor {
            &self.folders.editor_scroll
        } else {
            &self.folders.picker_scroll
        };
        let list_id = if for_editor {
            "editor-folders"
        } else {
            "folder-choices"
        };
        elevated_card().h(px(184.)).min_w_0().p(px(6.)).child(
            uniform_list(
                list_id,
                count,
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    let pal = theme::palette();
                    range
                        .filter_map(|index| {
                            let (id, label, full_name) = if index < special {
                                if !for_editor && index == 0 {
                                    (None, "所有文件夹".into(), "所有文件夹".into())
                                } else {
                                    (
                                        (!for_editor).then(String::new),
                                        "无文件夹".into(),
                                        "无文件夹".into(),
                                    )
                                }
                            } else {
                                let row = this.folders.rows.get(index - special)?;
                                let folder =
                                    this.client.overview.as_ref()?.folders.get(row.source)?;
                                (
                                    Some(folder.id.clone()),
                                    row.full_display.clone(),
                                    row.full_name.clone(),
                                )
                            };
                            let selected = if for_editor {
                                this.client
                                    .editor
                                    .as_ref()
                                    .and_then(|editor| editor.folder_id())
                                    == id.as_deref()
                            } else {
                                this.client.selected_folder == id
                            };
                            Some(
                                div()
                                    .id((list_id, index))
                                    .cursor_pointer()
                                    .h(px(32.))
                                    .w_full()
                                    .px(px(8.))
                                    .rounded(px(radius::CONTROL))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .when(selected, |r| {
                                        r.bg(pal.tint(pal.accent_fill))
                                            .border_1()
                                            .border_color(pal.tint_border(pal.accent_fill))
                                    })
                                    .when(!selected, |r| r.hover(move |s| s.bg(hex(pal.hover))))
                                    .child(
                                        div()
                                            .text_color(hex(if selected {
                                                pal.accent_text
                                            } else {
                                                pal.text_muted
                                            }))
                                            .child(glyph_icon(glyph::FOLDER, 12.)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(font::CAPTION))
                                            .font_weight(if selected {
                                                gpui::FontWeight::SEMIBOLD
                                            } else {
                                                gpui::FontWeight::NORMAL
                                            })
                                            .text_color(hex(pal.text))
                                            .child(label),
                                    )
                                    .when(selected, |r| {
                                        r.child(
                                            div()
                                                .text_color(hex(pal.accent_text))
                                                .child(glyph_icon(glyph::CHECK, 11.)),
                                        )
                                    })
                                    .tooltip(move |_, cx| make_tooltip(full_name.clone(), None, cx))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.lock_queued || !this.client.ready() {
                                            return;
                                        }
                                        if for_editor {
                                            if let Some(editor) = &mut this.client.editor {
                                                editor.choose_folder(id.clone());
                                                this.ui.animate_collapsible(
                                                    ElementId::Name("editor-folder-picker".into()),
                                                    false,
                                                    this.animate,
                                                );
                                            }
                                        } else {
                                            this.client.selected_folder = id.clone();
                                            this.client.show_folders = false;
                                            this.close_detail();
                                            this.invalidate_query();
                                            this.list_motion = this.transition();
                                        }
                                        cx.notify();
                                    })),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(scroll.clone())
            .size_full(),
        )
    }

    fn start_folder(&mut self, op: FolderOp, window: &mut Window, cx: &mut Context<Self>) {
        if !self.write_available() || self.folders.form.is_some() {
            return;
        }
        if let FolderOp::DeleteMany(ids) = &op {
            if ids.is_empty() || ids.iter().any(|id| !self.folders.by_id.contains_key(id)) {
                return;
            }
            if ids.len() > 500 {
                self.client.notice = "每次最多可删除 500 个文件夹。".into();
                cx.notify();
                return;
            }
        }
        if matches!(&op, FolderOp::Rename(id) | FolderOp::Delete(id) if !self.folders.by_id.contains_key(id))
        {
            return;
        }
        let initial = match &op {
            FolderOp::Rename(id) => self.folders.name(id, self.client.overview.as_ref()),
            _ => "",
        };
        let name = cx.new(|cx| {
            let mut field = Input::new("完整文件夹名，例如 工作/项目", false, cx);
            field.set(initial, cx);
            field
        });
        if !matches!(op, FolderOp::Delete(_) | FolderOp::DeleteMany(_)) {
            name.read(cx).focus(window);
        }
        self.folders.form = Some(FolderForm {
            op,
            name,
            must_reconcile: false,
        });
        cx.notify();
    }

    fn save_folder(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.folders.form.as_ref() else {
            return;
        };
        if form.must_reconcile {
            self.client.notice =
                "保存结果需核对。请先取消此表单并同步核对列表，再开始新的操作，避免重复提交。"
                    .into();
            cx.notify();
            return;
        }
        if !self.write_available() {
            return;
        }
        if !matches!(&form.op, FolderOp::Delete(_) | FolderOp::DeleteMany(_))
            && !form.name.read(cx).committed()
        {
            self.client.notice = "请先完成输入法组合，再保存文件夹名称。".into();
            cx.notify();
            return;
        }
        let name = form.name.read(cx).value().trim().to_owned();
        let op = form.op.clone();
        if matches!(&op, FolderOp::Rename(id) | FolderOp::Delete(id) if !self.folders.by_id.contains_key(id))
            || matches!(&op, FolderOp::DeleteMany(ids) if ids.is_empty() || ids.len() > 500 || ids.iter().any(|id| !self.folders.by_id.contains_key(id)))
        {
            self.client.notice = "所选文件夹已变化，请先刷新保险库后重试。".into();
            cx.notify();
            return;
        }
        if name.is_empty() && !matches!(op, FolderOp::Delete(_) | FolderOp::DeleteMany(_)) {
            self.client.notice = "请输入文件夹名称。".into();
            cx.notify();
            return;
        }
        let request = Request::begin();
        self.run(
            "正在保存文件夹",
            Some(request),
            cx,
            move || match op {
                FolderOp::Create => request.create_folder(&name),
                FolderOp::Rename(id) => request.rename_folder(&id, &name),
                FolderOp::Delete(id) => request.delete_folders(&[id]),
                FolderOp::DeleteMany(ids) => request.delete_folders(&ids),
            },
            |this, result, _| match result {
                Ok(res) if !this.lock_queued => {
                    this.folders.form = None;
                    this.folders.selected = res.id.clone().or_else(|| {
                        this.folders
                            .selected
                            .take()
                            .filter(|id| res.overview.folders.iter().any(|folder| &folder.id == id))
                    });
                    if this.client.selected_folder.as_ref().is_some_and(|id| {
                        !id.is_empty()
                            && !res.overview.folders.iter().any(|folder| &folder.id == id)
                    }) {
                        this.client.selected_folder = None;
                    }
                    this.folders.selected_ids.clear();
                    this.client.set_overview(res.overview);
                    this.client.write_guard = false;
                    this.client.overview_error = None;
                    this.invalidate_query();
                    this.session_notice();
                }
                Ok(_) => {}
                Err(err) => {
                    if let Some(form) = &mut this.folders.form {
                        form.record_error(err.code());
                    }
                    this.mark_write_uncertain(&err);
                }
            },
        );
    }

    fn browse_folder(&mut self, id: Option<String>, for_vault: bool, cx: &mut Context<Self>) {
        if self.lock_queued || !self.client.ready() || self.client.overview.is_none() {
            return;
        }
        if id
            .as_ref()
            .is_some_and(|id| !self.folders.by_id.contains_key(id))
        {
            return;
        }
        if for_vault {
            if let Some(id) = id {
                self.close_detail();
                self.client.selected_folder = Some(id);
                self.invalidate_query();
                self.list_motion = self.transition();
            } else {
                self.open_page(Page::Folders, cx);
                self.browse_folder(None, false, cx);
            }
        } else {
            let prev_depth = self.folders.manager.trail.len();
            self.folders
                .manager
                .select(id.as_deref(), &self.folders.by_id, &self.folders.rows);
            let next_depth = self.folders.manager.trail.len();
            self.folders.set_query("");
            self.folders.selected = id;
            self.folders.update_visible();
            self.folders.scroll = UniformListScrollHandle::new();
            if let Some(search) = &self.folders.search {
                search.update(cx, |input, cx| input.set("", cx));
            }
            self.ui
                .trigger_folder_nav(next_depth >= prev_depth, self.animate);
        }
        cx.notify();
    }

    fn render_folder_trail(
        &self,
        for_vault: bool,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{elevated_card, glyph_icon, make_tooltip},
            spring::clamp_opacity,
            tokens::{font, glyph, radius},
        };
        let pal = theme::palette();
        let navigation = if for_vault {
            &self.folders.vault
        } else {
            &self.folders.manager
        };
        let earlier = navigation.trail.len().saturating_sub(8);
        let scope = if for_vault {
            "vault-folder-crumb"
        } else {
            "manager-folder-crumb"
        };
        let earlier_key = ElementId::Name(format!("{scope}-earlier").into());
        let earlier_p =
            self.ui
                .collapsible_progress(&earlier_key, navigation.earlier_open, now, self.animate);

        let crumb_pill = |id: (&'static str, usize), label: gpui::SharedString, current: bool| {
            div()
                .id(id)
                .cursor_pointer()
                .h(px(24.))
                .px(px(7.))
                .rounded(px(radius::SM))
                .flex()
                .items_center()
                .text_size(px(font::CAPTION))
                .font_weight(if current {
                    gpui::FontWeight::SEMIBOLD
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(hex(if current {
                    pal.text
                } else {
                    pal.text_secondary
                }))
                .when(current, |el| el.bg(hex(pal.selected)))
                .when(!current, |el| {
                    el.hover(move |s| s.bg(hex(pal.hover)).text_color(hex(pal.text)))
                })
                .child(label)
        };

        let mut trail = div().flex().flex_wrap().items_center().gap(px(2.)).child(
            crumb_pill(
                (scope, 0usize),
                "我的文件夹".into(),
                navigation.id.is_none(),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.browse_folder(None, for_vault, cx))),
        );
        if earlier > 0 {
            trail = trail
                .child(
                    div()
                        .text_color(hex(pal.text_muted))
                        .child(glyph_icon(glyph::CHEVRON_RIGHT, 9.)),
                )
                .child(
                    crumb_pill(
                        (scope, 1usize),
                        format!("… ({earlier})").into(),
                        navigation.earlier_open,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let navigation = if for_vault {
                            &mut this.folders.vault
                        } else {
                            &mut this.folders.manager
                        };
                        navigation.earlier_open = !navigation.earlier_open;
                        let open = navigation.earlier_open;
                        this.ui.animate_collapsible(
                            ElementId::Name(format!("{scope}-earlier").into()),
                            open,
                            this.animate,
                        );
                        cx.notify();
                    })),
                );
        }
        for (index, row_index) in navigation.trail.iter().enumerate().skip(earlier) {
            let row = &self.folders.rows[*row_index];
            let Some(folder) = self
                .client
                .overview
                .as_ref()
                .and_then(|overview| overview.folders.get(row.source))
            else {
                continue;
            };
            let id = folder.id.clone();
            let full_name = row.full_name.clone();
            let is_leaf = index + 1 == navigation.trail.len();
            trail = trail
                .child(
                    div()
                        .text_color(hex(pal.text_muted))
                        .child(glyph_icon(glyph::CHEVRON_RIGHT, 9.)),
                )
                .child(
                    crumb_pill((scope, index + 2), row.display.clone(), is_leaf)
                        .max_w(px(180.))
                        .truncate()
                        .tooltip(move |_, cx| make_tooltip(full_name.clone(), None, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.browse_folder(Some(id.clone()), for_vault, cx)
                        })),
                );
        }
        div().flex().flex_col().gap(px(6.)).child(trail).when(
            earlier > 0 && (navigation.earlier_open || earlier_p > 0.02),
            |el| {
                el.child(
                    elevated_card()
                        .relative()
                        .top(px(-6. * (1. - earlier_p)))
                        .opacity(clamp_opacity(earlier_p))
                        .h(px(164.))
                        .p(px(6.))
                        .child(
                            uniform_list(
                                if for_vault {
                                    "vault-folder-ancestors"
                                } else {
                                    "manager-folder-ancestors"
                                },
                                earlier,
                                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                    let pal = theme::palette();
                                    let navigation = if for_vault {
                                        &this.folders.vault
                                    } else {
                                        &this.folders.manager
                                    };
                                    range
                                        .filter_map(|index| {
                                            let row = this
                                                .folders
                                                .rows
                                                .get(*navigation.trail.get(index)?)?;
                                            let folder = this
                                                .client
                                                .overview
                                                .as_ref()?
                                                .folders
                                                .get(row.source)?;
                                            let id = folder.id.clone();
                                            let full_name = row.full_name.clone();
                                            Some(
                                                div()
                                                    .id((scope, index + 10))
                                                    .cursor_pointer()
                                                    .h(px(30.))
                                                    .w_full()
                                                    .px(px(8.))
                                                    .rounded(px(radius::CONTROL))
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.))
                                                    .text_size(px(font::CAPTION))
                                                    .text_color(hex(pal.text))
                                                    .hover(move |s| s.bg(hex(pal.hover)))
                                                    .child(
                                                        div()
                                                            .text_color(hex(pal.accent_text))
                                                            .child(glyph_icon(glyph::FOLDER, 12.)),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .truncate()
                                                            .child(row.full_display.clone()),
                                                    )
                                                    .tooltip(move |_, cx| {
                                                        make_tooltip(full_name.clone(), None, cx)
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.browse_folder(
                                                                Some(id.clone()),
                                                                for_vault,
                                                                cx,
                                                            )
                                                        },
                                                    )),
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                }),
                            )
                            .track_scroll(navigation.scroll.clone())
                            .size_full(),
                        ),
                )
            },
        )
    }

    // Parent vault calls this before constructing its immutable list/detail views.
    // Compact single-line breadcrumb + horizontal child folder chips keep list height generous.
    pub(crate) fn render_folder_navigation(
        &mut self,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{glyph_icon, make_tooltip},
            tokens::{font, glyph, radius},
        };
        let pal = theme::palette();
        self.folders.vault.select(
            self.client.selected_folder.as_deref(),
            &self.folders.by_id,
            &self.folders.rows,
        );
        let Some(index) = self.folders.vault.trail.last().copied() else {
            return div();
        };
        let children = self.folders.children[index].clone();
        let count = children.len();
        div()
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(self.render_folder_trail(true, now, cx))
            .when(count > 0, |el| {
                let overview = self.client.overview.as_ref();
                el.child(
                    div()
                        .id("vault-child-folders-strip")
                        .w_full()
                        .overflow_x_scroll()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .pb(px(2.))
                        .children(children.iter().take(24).enumerate().filter_map(
                            |(i, &row_idx)| {
                                let row = self.folders.rows.get(row_idx)?;
                                let folder = overview?.folders.get(row.source)?;
                                let id = folder.id.clone();
                                let full_name = row.full_name.clone();
                                Some(
                                    div()
                                        .id(("vault-child-folder", i))
                                        .cursor_pointer()
                                        .h(px(25.))
                                        .px(px(8.))
                                        .rounded(px(radius::PILL))
                                        .bg(hex(pal.layer))
                                        .border_1()
                                        .border_color(hex(pal.border))
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .flex_shrink_0()
                                        .text_size(px(font::MICRO))
                                        .text_color(hex(pal.text_secondary))
                                        .hover(move |s| {
                                            s.bg(hex(pal.hover)).text_color(hex(pal.text))
                                        })
                                        .child(
                                            div()
                                                .text_color(hex(pal.accent_text))
                                                .child(glyph_icon(glyph::FOLDER, 10.)),
                                        )
                                        .child(row.display.clone())
                                        .tooltip(move |_, cx| {
                                            make_tooltip(full_name.clone(), None, cx)
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.browse_folder(Some(id.clone()), true, cx)
                                        })),
                                )
                            },
                        )),
                )
            })
    }

    /// Centered Spring Modal for creating, renaming, or deleting folders without replacing the list.
    pub(crate) fn render_folder_modal(&self, cx: &mut Context<Self>) -> gpui::Div {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{BtnVariant, btn, elevated_card, glyph_icon, info_bar, make_tooltip},
            toast::ToastKind,
            tokens::{font, glyph, radius, space},
        };
        let pal = theme::palette();
        let form = self.folders.form.as_ref().expect("folder form open");
        let is_del = matches!(form.op, FolderOp::Delete(_) | FolderOp::DeleteMany(_));
        let title = match form.op {
            FolderOp::Create => "新建文件夹",
            FolderOp::Rename(_) => "重命名文件夹",
            FolderOp::Delete(_) => "删除文件夹？",
            FolderOp::DeleteMany(_) => "删除所选文件夹？",
        };
        let target_label = match &form.op {
            FolderOp::Rename(id) | FolderOp::Delete(id) => self
                .folders
                .by_id
                .get(id)
                .map(|index| self.folders.rows[*index].full_name.clone()),
            FolderOp::DeleteMany(ids) => Some(format!("已明确选择 {} 个文件夹", ids.len()).into()),
            FolderOp::Create => None,
        };

        elevated_card()
            .p(px(20.))
            .w_full()
            .max_w(px(460.))
            .flex()
            .flex_col()
            .gap(px(space::M))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .size(px(34.))
                            .rounded(px(radius::PILL))
                            .bg(pal.tint(if is_del { pal.danger } else { pal.accent_fill }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(hex(if is_del { pal.danger } else { pal.accent_text }))
                            .child(glyph_icon(
                                if is_del { glyph::DELETE } else { glyph::FOLDER },
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
                                    .child(
                                        "修改不会级联；删除仅将该文件夹的条目移至「无文件夹」。",
                                    ),
                            ),
                    ),
            )
            .when_some(target_label, |el, full_name| {
                el.child(
                    div()
                        .id("folder-form-target")
                        .p(px(10.))
                        .rounded(px(radius::CONTROL))
                        .bg(hex(pal.layer))
                        .border_1()
                        .border_color(hex(pal.border_subtle))
                        .text_size(px(font::CAPTION))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(hex(pal.accent_text))
                        .child(compact_name(&full_name))
                        .tooltip(move |_, cx| make_tooltip(full_name.clone(), None, cx)),
                )
            })
            .when(!is_del, |el| el.child(form.name.clone()))
            .when(form.must_reconcile, |el| {
                el.child(info_bar(
                    ToastKind::Warning,
                    "保存结果需核对。请取消此表单，查看同步后的列表；此操作不会再次提交。",
                ))
            })
            .when(self.client.ready(), |el| {
                el.child(
                    div()
                        .pt(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .when(self.write_available() && !form.must_reconcile, |bar| {
                            bar.child(
                                btn(
                                    "save-folder",
                                    if is_del {
                                        BtnVariant::Danger
                                    } else {
                                        BtnVariant::Primary
                                    },
                                )
                                .flex_1()
                                .h(px(36.))
                                .child(if is_del { "确认删除" } else { "保存" })
                                .on_click(cx.listener(|this, _, _, cx| this.save_folder(cx))),
                            )
                        })
                        .child(
                            btn("cancel-folder", BtnVariant::Secondary)
                                .debug_selector(|| "cancel-folder".into())
                                .h(px(36.))
                                .px(px(16.))
                                .child("取消")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this.client.ready() {
                                        this.folders.form = None;
                                        cx.notify();
                                    }
                                })),
                        ),
                )
            })
    }

    pub(crate) fn render_folders(
        &mut self,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{
                BtnVariant, badge, btn, empty_state, glyph_icon, icon_btn, make_tooltip,
                surface_card,
            },
            spring::clamp_opacity,
            tokens::{font, glyph, radius, space},
        };

        let pal = theme::palette();
        let (nav_p, _) =
            self.ui
                .folder_nav_spring
                .sample_staggered(now, Duration::ZERO, self.animate);
        let nav_x = 14. * self.ui.folder_nav_dir * (1. - nav_p);

        let mut root = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .max_w(px(860.))
            .mx_auto()
            .p(px(space::L))
            .flex()
            .flex_col()
            .gap(px(space::M))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(space::M))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(font::SUBTITLE))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(hex(pal.text))
                                    .child("我的文件夹"),
                            )
                            .child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_secondary))
                                    .child("搜索所有层级的完整名称，或进入子文件夹整理层级结构。"),
                            ),
                    )
                    .when(
                        self.write_available() && self.folders.form.is_none(),
                        |el| {
                            let selected = self.folders.selected.clone();
                            let mut selected_ids = self
                                .folders
                                .selected_ids
                                .iter()
                                .cloned()
                                .collect::<Vec<_>>();
                            selected_ids.sort();
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        btn("new-folder", BtnVariant::Primary)
                                            .h(px(30.))
                                            .text_size(px(font::CAPTION))
                                            .child(glyph_icon(glyph::ADD, 12.))
                                            .child("新建文件夹")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.start_folder(FolderOp::Create, window, cx)
                                            })),
                                    )
                                    .when(!selected_ids.is_empty(), |bar| {
                                        let ids = selected_ids.clone();
                                        bar.child(
                                            btn("delete-selected-folders", BtnVariant::Danger)
                                                .h(px(30.))
                                                .text_size(px(font::CAPTION))
                                                .child(glyph_icon(glyph::DELETE, 12.))
                                                .child(format!("删除所选 ({})", ids.len()))
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.start_folder(
                                                            FolderOp::DeleteMany(ids.clone()),
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                )),
                                        )
                                    })
                                    .when_some(selected, |bar, id| {
                                        let rename = id.clone();
                                        bar.child(
                                            btn("rename-folder", BtnVariant::Secondary)
                                                .h(px(30.))
                                                .text_size(px(font::CAPTION))
                                                .child(glyph_icon(glyph::EDIT, 12.))
                                                .child("重命名")
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.start_folder(
                                                            FolderOp::Rename(rename.clone()),
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                )),
                                        )
                                        .child(
                                            btn("delete-folder", BtnVariant::Danger)
                                                .h(px(30.))
                                                .text_size(px(font::CAPTION))
                                                .child(glyph_icon(glyph::DELETE, 12.))
                                                .child("删除")
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.start_folder(
                                                            FolderOp::Delete(id.clone()),
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                )),
                                        )
                                    }),
                            )
                        },
                    ),
            );
        if self.client.overview.is_none() {
            return root.child(
                surface_card()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(empty_state(
                        glyph::FOLDER,
                        "解锁并同步后查看文件夹",
                        "连接账户并同步保险库后可在这里管理全部嵌套文件夹。",
                    )),
            );
        }
        if self.folders.search.is_none() {
            let search = cx.new(|cx| Input::new("搜索完整文件夹名称", false, cx));
            cx.observe(&search, |this, input, cx| {
                let input = input.read(cx);
                if input.committed() && this.folders.set_query(input.value()) {
                    cx.notify();
                }
            })
            .detach();
            self.folders.search = Some(search);
        }
        let count = self.folders.visible.len();
        root = root
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(space::M))
                    .child(
                        div()
                            .flex_1()
                            .max_w(px(420.))
                            .child(self.folders.search.as_ref().unwrap().clone()),
                    )
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(hex(pal.text_muted))
                            .child(if self.folders.query.is_empty() {
                                format!(
                                    "{count} 个当前层级 · 共 {} 个 · 已选 {} 个",
                                    self.folders.rows.len(),
                                    self.folders.selected_ids.len(),
                                )
                            } else {
                                format!(
                                    "{count} 个匹配 · 已选 {} 个",
                                    self.folders.selected_ids.len(),
                                )
                            }),
                    ),
            )
            .child(self.render_folder_trail(false, now, cx));

        root.child(
            surface_card()
                .relative()
                .left(px(nav_x))
                .opacity(clamp_opacity(0.45 + 0.55 * nav_p))
                .flex_1()
                .min_h_0()
                .p(px(6.))
                .when(count == 0, |el| {
                    el.flex().items_center().justify_center().child(empty_state(
                        glyph::FOLDER,
                        if self.folders.query.is_empty() {
                            "没有子文件夹"
                        } else {
                            "没有匹配的文件夹"
                        },
                        "点击右上角「新建文件夹」来整理你的保险库。",
                    ))
                })
                .when(count > 0, |el| {
                    el.child(
                        uniform_list(
                            "folder-manager",
                            count,
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                let pal = theme::palette();
                                range
                                    .filter_map(|index| {
                                        let row_index = *this.folders.visible.get(index)?;
                                        let row = this.folders.rows.get(row_index)?;
                                        let folder = this
                                            .client
                                            .overview
                                            .as_ref()?
                                            .folders
                                            .get(row.source)?;
                                        let id = folder.id.clone();
                                        let open_id = id.clone();
                                        let browse_id = id.clone();
                                        let rename_id = id.clone();
                                        let delete_id = id.clone();
                                        let full_name = row.full_name.clone();
                                        let label = if this.folders.query.is_empty() {
                                            row.display.clone()
                                        } else {
                                            row.full_display.clone()
                                        };
                                        let selected =
                                            this.folders.selected.as_deref() == Some(id.as_str());
                                        let bulk_selected = this.folders.selected_ids.contains(&id);
                                        let toggle_id = id.clone();
                                        let child_count = this.folders.children[row_index].len();
                                        let can_write = this.write_available();
                                        Some(
                                            div()
                                                .h(px(44.))
                                                .px(px(6.))
                                                .flex()
                                                .items_center()
                                                .gap(px(8.))
                                                .when(can_write, |row| {
                                                    row.child(
                                                        btn(
                                                            ("folder-bulk-select", index),
                                                            if bulk_selected {
                                                                BtnVariant::Tinted
                                                            } else {
                                                                BtnVariant::Subtle
                                                            },
                                                        )
                                                        .h(px(28.))
                                                        .w(px(28.))
                                                        .px_0()
                                                        .child(glyph_icon(
                                                            if bulk_selected {
                                                                glyph::CHECK
                                                            } else {
                                                                glyph::ADD
                                                            },
                                                            11.,
                                                        ))
                                                        .tooltip(move |_, cx| {
                                                            make_tooltip(
                                                                if bulk_selected {
                                                                    "取消批量选择"
                                                                } else {
                                                                    "加入批量删除选择"
                                                                },
                                                                None,
                                                                cx,
                                                            )
                                                        })
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                if !this.write_available() {
                                                                    return;
                                                                }
                                                                if !this
                                                                    .folders
                                                                    .toggle_bulk_selection(toggle_id.clone())
                                                                {
                                                                    this.client.notice = "每次最多可选择 500 个文件夹删除。".into();
                                                                    cx.notify();
                                                                    return;
                                                                }
                                                                cx.notify();
                                                            },
                                                        )),
                                                    )
                                                })
                                                .child(
                                                    div()
                                                        .id(("select-folder", index))
                                                        .cursor_pointer()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .h(px(36.))
                                                        .px(px(10.))
                                                        .rounded(px(radius::CONTROL))
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(8.))
                                                        .when(selected, |r| {
                                                            r.bg(pal.tint(pal.accent_fill))
                                                                .border_1()
                                                                .border_color(
                                                                    pal.tint_border(
                                                                        pal.accent_fill,
                                                                    ),
                                                                )
                                                        })
                                                        .when(!selected, |r| {
                                                            r.hover(move |s| s.bg(hex(pal.hover)))
                                                        })
                                                        .child(
                                                            div()
                                                                .text_color(hex(pal.accent_text))
                                                                .child(glyph_icon(
                                                                    glyph::FOLDER,
                                                                    14.,
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .min_w_0()
                                                                .truncate()
                                                                .text_size(px(font::BODY))
                                                                .text_color(hex(pal.text))
                                                                .child(label),
                                                        )
                                                        .tooltip(move |_, cx| {
                                                            make_tooltip(full_name.clone(), None, cx)
                                                        })
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                if this.lock_queued
                                                                    || !this.client.ready()
                                                                {
                                                                    return;
                                                                }
                                                                this.folders.selected =
                                                                    Some(id.clone());
                                                                cx.notify();
                                                            },
                                                        )),
                                                )
                                                .when(child_count > 0, |el| {
                                                    el.child(
                                                        btn(
                                                            ("browse-folder", index),
                                                            BtnVariant::Subtle,
                                                        )
                                                        .h(px(28.))
                                                        .px(px(6.))
                                                        .text_size(px(font::CAPTION))
                                                        .child(badge(
                                                            format!("{child_count} 子目录"),
                                                            pal.info,
                                                        ))
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.browse_folder(
                                                                    Some(browse_id.clone()),
                                                                    false,
                                                                    cx,
                                                                )
                                                            },
                                                        )),
                                                    )
                                                })
                                                .when(can_write, |el| {
                                                    el.child(
                                                        icon_btn(
                                                            ("row-rename-folder", index),
                                                            glyph::EDIT,
                                                            "重命名文件夹",
                                                            None,
                                                        )
                                                        .size(px(28.))
                                                        .on_click(cx.listener(
                                                            move |this, _, window, cx| {
                                                                this.start_folder(
                                                                    FolderOp::Rename(
                                                                        rename_id.clone(),
                                                                    ),
                                                                    window,
                                                                    cx,
                                                                )
                                                            },
                                                        )),
                                                    )
                                                    .child(
                                                        icon_btn(
                                                            ("row-delete-folder", index),
                                                            glyph::DELETE,
                                                            "删除文件夹",
                                                            None,
                                                        )
                                                        .size(px(28.))
                                                        .on_click(cx.listener(
                                                            move |this, _, window, cx| {
                                                                this.start_folder(
                                                                    FolderOp::Delete(
                                                                        delete_id.clone(),
                                                                    ),
                                                                    window,
                                                                    cx,
                                                                )
                                                            },
                                                        )),
                                                    )
                                                })
                                                .child(
                                                    btn(
                                                        ("open-folder", index),
                                                        BtnVariant::Secondary,
                                                    )
                                                    .h(px(28.))
                                                    .px(px(10.))
                                                    .text_size(px(font::CAPTION))
                                                    .child("打开")
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        if this.lock_queued || !this.client.ready()
                                                        {
                                                            return;
                                                        }
                                                        this.open_page(Page::Vault, cx);
                                                        this.client.selected_folder =
                                                            Some(open_id.clone());
                                                        this.invalidate_query();
                                                        this.ui.trigger_list_motion(this.animate);
                                                        cx.notify();
                                                    })),
                                                ),
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(self.folders.scroll.clone())
                        .size_full(),
                    )
                }),
        )
    }
}

#[cfg(test)]
#[path = "folder_navigation_tests.rs"]
mod navigation_tests;
#[cfg(all(test, feature = "test-support"))]
#[path = "folder_workflow_tests.rs"]
mod workflow_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bulk_folder_selection_is_id_based_bounded_and_pruned_after_refresh() {
        let mut folders = Folders::default();
        assert!(folders.toggle_bulk_selection("old".into()));
        assert!(folders.toggle_bulk_selection("keep".into()));
        assert!(folders.toggle_bulk_selection("keep".into()));
        assert!(!folders.selected_ids.contains("keep"));

        let overview = |ids: &[&str]| Overview {
            items: Vec::new(),
            folders: ids
                .iter()
                .map(|id| Folder {
                    id: (*id).into(),
                    name: (*id).into(),
                })
                .collect(),
            last_sync: String::new(),
            failures: 0,
            read_only: false,
        };
        folders.refresh(1, Some(&overview(&["old", "keep"])));
        assert!(folders.selected_ids.contains("old"));
        assert!(!folders.selected_ids.contains("keep"));
        folders.refresh(2, Some(&overview(&["keep"])));
        assert!(folders.selected_ids.is_empty());

        for index in 0..500 {
            assert!(folders.toggle_bulk_selection(format!("id-{index}")));
        }
        assert!(!folders.toggle_bulk_selection("overflow".into()));
        assert_eq!(folders.selected_ids.len(), 500);
    }

    #[test]
    fn folder_labels_keep_real_ancestors_across_prefix_collisions_and_duplicate_paths() {
        let names = [
            "A/B/C",
            "A!",
            "A",
            "A/B",
            "A/BC",
            "A/B",
            "/missing/leaf/",
            "😀/甲",
        ];
        let folders = names
            .iter()
            .enumerate()
            .map(|(i, name)| Folder {
                id: i.to_string(),
                name: (*name).into(),
            })
            .collect::<Vec<_>>();
        let rows = labels(&folders);
        let depth_by_source = rows
            .iter()
            .map(|row| (row.source, row.depth))
            .collect::<HashMap<_, _>>();
        assert_eq!(depth_by_source[&0], 2);
        assert_eq!(depth_by_source[&1], 0);
        assert_eq!(depth_by_source[&2], 0);
        assert_eq!(depth_by_source[&3], 1);
        assert_eq!(depth_by_source[&4], 1);
        assert_eq!(depth_by_source[&5], 1);
        assert_eq!(depth_by_source[&6], 0);
        assert_eq!(depth_by_source[&7], 0);
        assert_eq!(rows.len(), folders.len());
    }

    #[test]
    fn deep_folder_projection_is_iterative_and_keeps_the_last_real_path() {
        let mut path = String::new();
        let folders = (0..1500)
            .map(|i| {
                if !path.is_empty() {
                    path.push('/');
                }
                path.push('甲');
                Folder {
                    id: i.to_string(),
                    name: path.clone(),
                }
            })
            .collect::<Vec<_>>();
        let rows = labels(&folders);
        assert_eq!(rows.len(), 1500);
        assert_eq!(rows.last().unwrap().source, 1499);
        assert_eq!(rows.last().unwrap().depth, 1499);
    }

    #[test]
    fn folder_labels_do_not_fake_ancestors() {
        let folders = [
            Folder {
                id: "1".into(),
                name: "A".into(),
            },
            Folder {
                id: "2".into(),
                name: "A/B/C".into(),
            },
            Folder {
                id: "3".into(),
                name: "X/Y".into(),
            },
        ];
        assert_eq!(
            labels(&folders)
                .iter()
                .map(|row| row.depth)
                .collect::<Vec<_>>(),
            [0, 1, 0]
        );
    }
}
