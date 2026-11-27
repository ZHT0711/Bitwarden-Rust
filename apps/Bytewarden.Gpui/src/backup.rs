use bytewarden_core::desktop::{ExportFormat, Request};
use gpui::{Context, Entity, div, prelude::*, px};

use crate::{
    Page, VaultPreview,
    detail_tools::{FileChooser, choose_file},
    input::Input,
};

pub struct Backup {
    mode: Mode,
    master: Entity<Input>,
    protection: Entity<Input>,
    plaintext_confirmed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Import,
    Export(ExportFormat),
}

const MODES: &[(Mode, &str)] = &[
    (Mode::Import, "导入"),
    (Mode::Export(ExportFormat::PasswordJson), "密码保护 JSON"),
    (Mode::Export(ExportFormat::AccountJson), "当前账户加密 JSON"),
    (Mode::Export(ExportFormat::PlainJson), "明文 JSON"),
    (Mode::Export(ExportFormat::Csv), "明文 CSV"),
];

impl Backup {
    pub fn new(cx: &mut Context<VaultPreview>) -> Self {
        Self {
            mode: Mode::Import,
            master: cx.new(|cx| Input::new("当前账户主密码", true, cx)),
            protection: cx.new(|cx| Input::new("文件密码", true, cx)),
            plaintext_confirmed: false,
        }
    }
    fn credentials_committed(&self, mode: Mode, cx: &gpui::App) -> bool {
        match mode {
            Mode::Import => self.protection.read(cx).committed(),
            Mode::Export(ExportFormat::PasswordJson) => {
                self.master.read(cx).committed() && self.protection.read(cx).committed()
            }
            Mode::Export(_) => self.master.read(cx).committed(),
        }
    }

    pub fn clear(&mut self, cx: &mut Context<VaultPreview>) {
        self.master.update(cx, |input, cx| input.set("", cx));
        self.protection.update(cx, |input, cx| input.set("", cx));
        self.plaintext_confirmed = false;
    }
}

fn plaintext(mode: Mode) -> bool {
    matches!(
        mode,
        Mode::Export(ExportFormat::PlainJson | ExportFormat::Csv)
    )
}

impl VaultPreview {
    fn choose_backup_file(&mut self, cx: &mut Context<Self>) {
        let mode = self.backup.mode;
        if !self.client.sync_available() || (mode == Mode::Import && self.client.write_guard) {
            return;
        }
        if !self.backup.credentials_committed(mode, cx) {
            self.client.notice = "请先完成密码输入法组合，再继续。".into();
            cx.notify();
            return;
        }
        if plaintext(mode) && !self.backup.plaintext_confirmed {
            return;
        }
        if matches!(mode, Mode::Export(_)) && self.backup.master.read(cx).value().is_empty() {
            self.client.notice = "导出需要当前账户主密码。".into();
            cx.notify();
            return;
        }
        if mode == Mode::Export(ExportFormat::PasswordJson)
            && self.backup.protection.read(cx).value().is_empty()
        {
            self.client.notice = "请输入用于保护导出文件的独立密码。".into();
            cx.notify();
            return;
        }
        let (chooser, name) = match mode {
            Mode::Import => (FileChooser::Open, None),
            Mode::Export(ExportFormat::Csv) => (FileChooser::Save, Some("bytewarden-vault.csv")),
            Mode::Export(_) => (FileChooser::Save, Some("bytewarden-vault.json")),
        };
        let account = self.client.active().expect("online account").id.clone();
        choose_file(
            cx,
            self.client.generation,
            chooser,
            name,
            move |this, path, cx| {
                if this.page != Page::Backup
                    || this.backup.mode != mode
                    || !this.client.sync_available()
                    || (mode == Mode::Import && this.client.write_guard)
                    || !this.backup.credentials_committed(mode, cx)
                    || this
                        .client
                        .active()
                        .is_none_or(|active| active.id != account)
                {
                    return;
                }
                let confirmed = this.backup.plaintext_confirmed;
                if plaintext(mode) && !confirmed {
                    return;
                }
                let master = this.backup.master.update(cx, |input, cx| input.take(cx));
                let protection = this
                    .backup
                    .protection
                    .update(cx, |input, cx| input.take(cx));
                this.backup.plaintext_confirmed = false;
                let request = Request::begin();
                this.run("正在处理保险库文件",Some(request),cx,move||match mode{
                Mode::Import=>request.import(&account,&path,&protection).map(Report::Imported),
                Mode::Export(format)=>request.export(&account,&path,format,confirmed,&master,&protection).map(Report::Exported),
            },|this,result,_|match result {
                Ok(Report::Imported(result)) if !this.lock_queued=>{
                    this.close_detail();
                    this.client.set_overview(result.overview);
                    this.client.write_guard = false;
                    this.client.overview_error = None;
                    this.invalidate_query();
                    this.client.notice = "保险库已导入。".into();
                }
                Ok(Report::Exported(report))=>{this.client.notice=format!("已导出 {} 个条目和 {} 个文件夹；跳过受限项 {} 个、不支持的条目 {} 个，未包含 {} 个附件文件。",report.items,report.folders,report.skipped_restricted,report.omitted_unsupported,report.omitted_attachments);}
                Ok(Report::Imported(_))=>{},
                Err(e)=>this.mark_write_uncertain(&e),
            });
            },
        );
    }

    pub fn render_backup(&self, now: std::time::Instant, cx: &Context<Self>) -> impl IntoElement {
        use crate::theme::{self, hex};
        use crate::ui::{
            components::{
                BtnVariant, btn, card_group, glyph_icon, info_bar, pill_chip,
                toggle_switch_animated,
            },
            toast::ToastKind,
            tokens::{font, glyph, space},
        };

        let pal = theme::palette();
        let mode = self.backup.mode;
        let plain_p = self.ui.toggle_progress(
            &gpui::ElementId::Name("allow-plaintext".into()),
            self.backup.plaintext_confirmed,
            now,
            self.animate,
        );
        div()
            .id("backup-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(space::M))
            .max_w(px(760.))
            .child(
                card_group("导入与导出个人保险库", Some(glyph::EXPORT)).child(
                    div()
                        .p(px(space::L))
                        .flex()
                        .flex_col()
                        .gap(px(space::M))
                        .child(
                            div()
                                .text_size(px(font::CAPTION))
                                .text_color(hex(pal.text_secondary))
                                .child("支持 Bitwarden JSON 和 CSV。导出不包含附件文件，也不能替代独立备份。"),
                        )
                        .child(
                            div().flex().flex_wrap().gap(px(6.)).children(
                                MODES
                                    .iter()
                                    .copied()
                                    .enumerate()
                                    .map(|(i, (choice, label))| {
                                        pill_chip(("backup-mode", i), label, None, mode == choice)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if this.client.ready() {
                                                    this.backup.clear(cx);
                                                    this.backup.mode = choice;
                                                    cx.notify();
                                                }
                                            }))
                                    }),
                            ),
                        )
                        .when(matches!(mode, Mode::Export(_)), |el| {
                            el.child(self.backup.master.clone())
                        })
                        .when(
                            matches!(
                                mode,
                                Mode::Import | Mode::Export(ExportFormat::PasswordJson)
                            ),
                            |el| el.child(self.backup.protection.clone()),
                        )
                        .when(mode == Mode::Import, |el| {
                            el.child(
                                div()
                                    .text_size(px(font::CAPTION))
                                    .text_color(hex(pal.text_muted))
                                    .child("导入明文或账户限定备份时，文件密码留空。导入只新增条目。"),
                            )
                        })
                        .when(plaintext(mode), |el| {
                            el.child(info_bar(
                                ToastKind::Warning,
                                "注意：明文文件会暴露所有密码，在其他软件中打开后也可能留下副本。",
                            ))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .py(px(4.))
                                    .child(
                                        div()
                                            .text_size(px(font::BODY))
                                            .text_color(hex(pal.text))
                                            .child("我已了解风险，确认导出明文"),
                                    )
                                    .child(
                                        toggle_switch_animated(
                                            "allow-plaintext",
                                            self.backup.plaintext_confirmed,
                                            plain_p,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                if this.client.ready() {
                                                    this.backup.plaintext_confirmed =
                                                        !this.backup.plaintext_confirmed;
                                                    let next = this.backup.plaintext_confirmed;
                                                    this.ui.animate_toggle(
                                                        "allow-plaintext",
                                                        next,
                                                        this.animate,
                                                    );
                                                    cx.notify();
                                                }
                                            }),
                                        ),
                                    ),
                            )
                        })
                        .when(
                            self.client.sync_available()
                                && (!plaintext(mode) || self.backup.plaintext_confirmed),
                            |el| {
                                el.child(
                                    btn("backup-file", BtnVariant::Primary)
                                        .h(px(36.))
                                        .child(glyph_icon(
                                            if mode == Mode::Import {
                                                glyph::UPLOAD
                                            } else {
                                                glyph::DOWNLOAD
                                            },
                                            14.,
                                        ))
                                        .child(if mode == Mode::Import {
                                            "选择文件并导入"
                                        } else {
                                            "选择保存位置并导出"
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.choose_backup_file(cx)
                                        })),
                                )
                            },
                        )
                        .when(!self.client.sync_available(), |el| {
                            el.child(info_bar(
                                ToastKind::Info,
                                "请登录并解锁在线保险库后再执行导入或导出。",
                            ))
                        }),
                ),
            )
    }
}

enum Report {
    Imported(bytewarden_core::desktop::Mutation),
    Exported(bytewarden_core::desktop::ExportReport),
}
