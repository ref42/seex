use crate::library_tools::LibraryInspection;
use crate::text_export::Format;
use crate::ui::{app::SeexUi, components, locale, theme};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::text::TextView;
use gpui_kit::*;
use std::sync::Arc;

#[derive(Default)]
pub struct LibraryState {
    pub running: bool,
    pub report: String,
    pub notice: String,
    pub inspection: Option<Arc<LibraryInspection>>,
}

impl SeexUi {
    pub(crate) fn libraries_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (running, report, notice, can_export) = self
            .library_state
            .lock()
            .map(|s| {
                (
                    s.running,
                    s.report.clone(),
                    s.notice.clone(),
                    s.inspection.is_some(),
                )
            })
            .unwrap_or_default();
        let results = TextView::markdown(
            "library-results-text",
            crate::text_export::selectable_text(&report),
        )
        .selectable(true)
        .scrollable(false);
        let status = if running {
            locale::t("Working…", "处理中…").to_owned()
        } else {
            notice
        };
        let status = TextView::markdown(
            "library-status-text",
            crate::text_export::selectable_text(&status),
        )
        .selectable(true)
        .scrollable(false);
        div().flex().flex_col().flex_1().min_h(px(0.0)).w_full().gap(px(16.0))
            .child(components::section_header(locale::t("Libraries", "库管理"),
                locale::t("Inspect existing libraries or merge a folder into a new library set.", "查看现有库，或将文件夹中的库合并为新库。")))
            .child(div().flex().flex_shrink_0().flex_wrap().gap(px(12.0))
                .child(components::button("inspect-library", locale::t("Inspect library", "查看库"), !running,
                    cx.listener(|this, _: &ClickEvent, _, cx| { this.start_library_tool(false); cx.notify(); })))
                .child(components::button("merge-libraries", locale::t("Merge libraries", "合并库"), !running,
                    cx.listener(|this, _: &ClickEvent, _, cx| { this.start_library_tool(true); cx.notify(); })))
                .child(components::button("export-library-csv", locale::t("Export CSV", "导出 CSV"), !running && can_export,
                    cx.listener(|this, _: &ClickEvent, _, cx| { this.export_library(Format::Csv); cx.notify(); })))
                .child(components::button("export-library-markdown", locale::t("Export Markdown", "导出 Markdown"), !running && can_export,
                    cx.listener(|this, _: &ClickEvent, _, cx| { this.export_library(Format::Markdown); cx.notify(); })))
                .child(components::button("copy-library-report", locale::t("Copy all", "复制全部"), !report.is_empty(),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        let report = this.library_state.lock().map(|s| s.report.clone()).unwrap_or_default();
                        cx.write_to_clipboard(ClipboardItem::new_string(report));
                    }))))
            .child(div().flex_shrink_0().text_size(px(20.0)).text_color(theme::muted()).child(locale::t(
                "Supports Altium SchLib/PcbLib and modern KiCad symbols, .pretty and .3dshapes folders. Conflicting names stop the merge. Choose a new output name; existing outputs are protected.",
                "支持 Altium SchLib/PcbLib 和现代 KiCad 符号、.pretty 与 .3dshapes 文件夹。同名内容冲突时停止合并。请选择新的输出名称，已有输出不会被覆盖。")))
            .child(div().flex_shrink_0().text_color(theme::muted()).child(status))
            .child(div().id("library-results-scroll").flex_1().min_h(px(0.0)).w_full()
                .text_size(px(theme::scaled_font(20.0)))
                .overflow_y_scrollbar().child(results))
            .into_any_element()
    }

    fn start_library_tool(&self, merge: bool) {
        let Ok(mut state) = self.library_state.lock() else {
            return;
        };
        if state.running {
            return;
        }
        state.running = true;
        state.notice.clear();
        drop(state);
        let state = self.library_state.clone();
        std::thread::spawn(move || {
            let result = if merge {
                rfd::FileDialog::new()
                    .set_title("Select source library folder")
                    .pick_folder()
                    .and_then(|input| {
                        rfd::FileDialog::new()
                            .set_title("Choose a NEW output base name")
                            .set_file_name("MergedLibrary")
                            .save_file()
                            .map(|output| {
                                crate::library_tools::merge(&input, &output)
                                    .map(|report| (report, None))
                            })
                    })
            } else {
                rfd::FileDialog::new()
                    .add_filter(
                        "EDA libraries",
                        &["SchLib", "PcbLib", "kicad_sym", "kicad_mod"],
                    )
                    .pick_file()
                    .map(|path| {
                        crate::library_tools::read_library(&path)
                            .map(|inspection| (inspection.report(), Some(Arc::new(inspection))))
                    })
            };
            if let Ok(mut state) = state.lock() {
                state.running = false;
                match result {
                    Some(Ok((report, inspection))) => {
                        state.report = report;
                        state.inspection = inspection;
                    }
                    Some(Err(error)) => state.notice = format!("Error: {error}"),
                    None => (), // Preserve the last successful result on cancel.
                }
            }
        });
    }

    fn export_library(&self, format: Format) {
        let Ok(mut state) = self.library_state.lock() else {
            return;
        };
        if state.running {
            return;
        }
        let Some(inspection) = state.inspection.clone() else {
            return;
        };
        state.running = true;
        state.notice.clear();
        drop(state);
        let state = self.library_state.clone();
        let exported = locale::t("Exported", "已导出").to_owned();
        std::thread::spawn(move || {
            let filename = format!(
                "{}-inventory.{}",
                inspection
                    .source
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy(),
                format.extension()
            );
            let result = rfd::FileDialog::new()
                .set_title(format!("Export library {}", format.label()))
                .add_filter(format.label(), &[format.extension()])
                .set_file_name(filename)
                .save_file()
                .map(|path| {
                    inspection
                        .export(&path, format)
                        .map(|()| format!("{exported} {}: {}", format.label(), path.display()))
                });
            if let Ok(mut state) = state.lock() {
                state.running = false;
                match result {
                    Some(Ok(message)) => state.notice = message,
                    Some(Err(error)) => state.notice = format!("Error: {error}"),
                    None => (),
                }
            }
        });
    }
}
