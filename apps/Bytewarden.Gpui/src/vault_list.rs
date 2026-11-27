use std::cmp::Ordering;

use bytewarden_core::desktop::Item;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Favorites,
    Kind(u8),
    Archived,
    Trash,
}

pub const FILTERS: &[(Filter, &str, &str)] = &[
    (Filter::All, "我的保险库", "\u{e80f}"),
    (Filter::Favorites, "收藏夹", "\u{e734}"),
    (Filter::Kind(1), "登录", "\u{e774}"),
    (Filter::Kind(2), "安全笔记", "\u{e8a5}"),
    (Filter::Kind(3), "银行卡", "\u{e8c7}"),
    (Filter::Kind(4), "身份信息", "\u{e8d4}"),
    (Filter::Kind(5), "SSH 密钥", "\u{e756}"),
    (Filter::Kind(6), "银行账户", "\u{e825}"),
    (Filter::Kind(7), "驾驶执照", "\u{e804}"),
    (Filter::Kind(8), "护照", "\u{e909}"),
    (Filter::Archived, "归档", "\u{e7b8}"),
    (Filter::Trash, "回收站", "\u{e74d}"),
];

pub fn icon_for(item: &Item) -> &'static str {
    FILTERS
        .iter()
        .find(|(filter, _, _)| *filter == Filter::Kind(item.item_type))
        .map_or("\u{e8a5}", |(_, _, icon)| *icon)
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum Sort {
    #[default]
    NameAsc,
    NameDesc,
    Recent,
    Oldest,
}
impl Sort {
    pub fn key(self) -> &'static str {
        match self {
            Self::NameAsc => "name-asc",
            Self::NameDesc => "name-desc",
            Self::Recent => "recent",
            Self::Oldest => "oldest",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "name-desc" => Self::NameDesc,
            "recent" => Self::Recent,
            "oldest" => Self::Oldest,
            _ => Self::NameAsc,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NameAsc => "名称（A → Z）",
            Self::NameDesc => "名称（Z → A）",
            Self::Recent => "最近修改",
            Self::Oldest => "最早修改",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::NameAsc => Self::NameDesc,
            Self::NameDesc => Self::Recent,
            Self::Recent => Self::Oldest,
            Self::Oldest => Self::NameAsc,
        }
    }
}

/// Normalized display metadata and one lazily selected order per vault snapshot.
/// Query changes only filter indices; item IDs and secret detail never enter search.
#[derive(Default)]
pub struct Index {
    keys: Vec<(String, String)>,
    order: Vec<usize>,
    sort: Option<Sort>,
}

impl Index {
    pub fn new(items: &[Item]) -> Self {
        Self {
            keys: items
                .iter()
                .map(|item| (item.name.to_lowercase(), item.subtitle.to_lowercase()))
                .collect(),
            order: (0..items.len()).collect(),
            sort: None,
        }
    }

    pub fn visible(
        &mut self,
        items: &[Item],
        filter: Filter,
        folder: Option<&str>,
        sort: Sort,
        query: &str,
    ) -> Vec<usize> {
        if self.sort != Some(sort) {
            self.order.sort_unstable_by(|i, j| {
                let (a, b) = (&items[*i], &items[*j]);
                let name = self.keys[*i].0.cmp(&self.keys[*j].0);
                let order = match sort {
                    Sort::NameAsc => name,
                    Sort::NameDesc => name.reverse(),
                    Sort::Recent => date_order(&b.revision_date, &a.revision_date).then(name),
                    Sort::Oldest => date_order(&a.revision_date, &b.revision_date).then(name),
                };
                order.then(a.id.cmp(&b.id)).then(i.cmp(j))
            });
            self.sort = Some(sort);
        }
        let terms = terms(query);
        self.order
            .iter()
            .copied()
            .filter(|index| {
                let item = &items[*index];
                category(item, filter)
                    && folder.is_none_or(|folder| item.folder_id.as_deref().unwrap_or("") == folder)
                    && terms.iter().all(|word| {
                        let (name, subtitle) = &self.keys[*index];
                        name.contains(word) || subtitle.contains(word)
                    })
            })
            .collect()
    }
}

#[cfg(test)]
fn visible(
    items: &[Item],
    filter: Filter,
    folder: Option<&str>,
    sort: Sort,
    query: &str,
) -> Vec<usize> {
    Index::new(items).visible(items, filter, folder, sort, query)
}

fn date_order(a: &str, b: &str) -> Ordering {
    a.cmp(b)
}

fn category(item: &Item, filter: Filter) -> bool {
    if filter == Filter::Trash {
        return item.deleted;
    }
    if item.deleted {
        return false;
    }
    match filter {
        Filter::Archived => item.archived,
        Filter::Favorites => item.favorite,
        Filter::All => !item.archived,
        Filter::Kind(kind) => !item.archived && kind == item.item_type,
        Filter::Trash => unreachable!(),
    }
}

fn terms(query: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut quoted = false;
    let mut buffer = String::new();
    for ch in query.chars() {
        if ch == '"' || (!quoted && ch.is_whitespace()) {
            if !buffer.is_empty() {
                result.push(std::mem::take(&mut buffer).to_lowercase());
            }
            if ch == '"' {
                quoted = !quoted;
            }
        } else {
            buffer.push(ch);
        }
    }
    if !buffer.is_empty() {
        result.push(buffer.to_lowercase());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(id: &str, name: &str, subtitle: &str) -> Item {
        Item {
            id: id.into(),
            name: name.into(),
            subtitle: subtitle.into(),
            item_type: 1,
            favorite: false,
            folder_id: None,
            organization_id: None,
            deleted: false,
            archived: false,
            reprompt: false,
            restricted: false,
            revision_date: "2026-11-25T00:00:00Z".into(),
            website_host: None,
        }
    }
    #[test]
    fn ten_thousand_items_keep_order_and_exact_search_results() {
        let items = (0..10_000)
            .map(|i| {
                let mut row = item(
                    &format!("id-{i:05}"),
                    &format!("条目 {:05}", (i * 7919) % 10_000),
                    "example.com 测试账户",
                );
                row.revision_date = format!("2026-11-{:02}T00:00:00Z", i % 28 + 1);
                row
            })
            .collect::<Vec<_>>();
        let start = std::time::Instant::now();
        for sort in [Sort::NameAsc, Sort::NameDesc, Sort::Recent, Sort::Oldest] {
            let ordered = visible(&items, Filter::All, None, sort, "");
            assert_eq!(ordered.len(), items.len());
            let mut unique = ordered.clone();
            unique.sort_unstable();
            assert_eq!(unique, (0..items.len()).collect::<Vec<_>>());
            for pair in ordered.windows(2) {
                let (a, b) = (&items[pair[0]], &items[pair[1]]);
                let names = a.name.to_lowercase().cmp(&b.name.to_lowercase());
                let comparison = match sort {
                    Sort::NameAsc => names,
                    Sort::NameDesc => names.reverse(),
                    Sort::Recent => b.revision_date.cmp(&a.revision_date).then(names),
                    Sort::Oldest => a.revision_date.cmp(&b.revision_date).then(names),
                }
                .then(a.id.cmp(&b.id));
                assert!(comparison != Ordering::Greater);
            }
            let found = visible(&items, Filter::All, None, sort, "EXAMPLE 09999");
            assert_eq!(found.len(), 1);
            assert_eq!(items[found[0]].name, "条目 09999");
        }
        eprintln!(
            "10k four sorts, exact searches and assertions: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn cached_search_reuses_sort_order_across_queries_and_filter_changes() {
        let mut items = vec![
            item("z", "Zulu", "ALPHA 中文"),
            item("a", "alpha", "😀 中文"),
            item("b", "Alpha", "other"),
            item("i", "İstanbul", "example"),
        ];
        items[0].folder_id = Some("nested-folder".into());
        items[0].favorite = true;
        items[2].deleted = true;
        items[1].revision_date = "2026-11-26T00:00:00Z".into();
        let mut index = Index::new(&items);
        assert_eq!(
            index.visible(&items, Filter::All, None, Sort::NameAsc, ""),
            [1, 3, 0]
        );
        assert_eq!(
            index.visible(&items, Filter::All, None, Sort::NameAsc, "ALPHA 中文"),
            [1, 0]
        );
        assert_eq!(
            index.visible(&items, Filter::All, None, Sort::NameAsc, "😀"),
            [1]
        );
        assert_eq!(
            index.visible(
                &items,
                Filter::Favorites,
                Some("nested-folder"),
                Sort::NameAsc,
                "alpha"
            ),
            [0]
        );
        assert_eq!(
            index.visible(&items, Filter::Trash, None, Sort::NameAsc, "alpha"),
            [2]
        );
        assert_eq!(
            index.visible(&items, Filter::All, None, Sort::NameDesc, ""),
            [0, 3, 1]
        );
        assert_eq!(
            index.visible(&items, Filter::All, None, Sort::Recent, ""),
            [1, 3, 0]
        );
        assert_eq!(
            index.visible(&items, Filter::All, None, Sort::NameAsc, "i\u{307}STANBUL"),
            [3]
        );
        assert_eq!(
            index.visible(&items, Filter::All, None, Sort::NameAsc, ""),
            [1, 3, 0]
        );
        for sort in [Sort::NameAsc, Sort::NameDesc, Sort::Recent, Sort::Oldest] {
            assert_eq!(Sort::parse(sort.key()), sort);
        }
        assert_eq!(Sort::parse("future-setting"), Sort::NameAsc);
    }

    #[test]
    fn metadata_filters_compose_without_searching_ids() {
        let a = item("internal", "Team Alpha", "开发");
        let mut b = item("second", "Alpha Notes", "公开");
        b.folder_id = Some("folder".into());
        b.deleted = true;
        let items = [a, b];
        assert_eq!(
            visible(&items, Filter::All, None, Sort::NameAsc, "ALPHA 开发"),
            [0]
        );
        assert_eq!(
            visible(&items, Filter::All, None, Sort::NameAsc, "\"team alpha\""),
            [0]
        );
        assert!(visible(&items, Filter::All, None, Sort::NameAsc, "internal").is_empty());
        assert_eq!(
            visible(
                &items,
                Filter::Trash,
                Some("folder"),
                Sort::NameAsc,
                "alpha"
            ),
            [1]
        );
    }
}
