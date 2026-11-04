use crate::ui::{app::SeexUi, components, locale, theme};
use gpui_kit::*;

#[derive(Default)]
pub struct LibraryState {
    pub running: bool,
    pub report: String,
}

impl SeexUi {
    pub(crate) fn libraries_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (running, report) = self
            .library_state
            .lock()
            .map(|s| (s.running, s.report.clone()))
            .unwrap_or_default();
        div().flex().flex_col().w_full().gap(px(16.0))
            .child(components::section_header(locale::t("Libraries", "库管理"),
                locale::t("Inspect existing libraries or merge a folder into a new library set.", "查看现有库，或将文件夹中的库合并为新库。")))
            .child(div().flex().gap(px(12.0))
                .child(components::button("inspect-library", locale::t("Inspect library", "查看库"), !running,
                    cx.listener(|this, _: &ClickEvent, _, cx| { this.start_library_tool(false); cx.notify(); })))
                .child(components::button("merge-libraries", locale::t("Merge libraries", "合并库"), !running,
                    cx.listener(|this, _: &ClickEvent, _, cx| { this.start_library_tool(true); cx.notify(); }))))
            .child(div().text_size(px(20.0)).text_color(theme::muted()).child(locale::t(
                "Supports Altium SchLib/PcbLib and modern KiCad symbols, .pretty and .3dshapes folders. Conflicting names stop the merge. Choose a new output name; existing outputs are protected.",
                "支持 Altium SchLib/PcbLib 和现代 KiCad 符号、.pretty 与 .3dshapes 文件夹。同名内容冲突时停止合并。请选择新的输出名称，已有输出不会被覆盖。")))
            .child(div().text_size(px(20.0)).child(if running { locale::t("Working…", "处理中…").to_owned() } else { report }))
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
                            .map(|output| crate::library_tools::merge(&input, &output))
                    })
            } else {
                rfd::FileDialog::new()
                    .add_filter(
                        "EDA libraries",
                        &["SchLib", "PcbLib", "kicad_sym", "kicad_mod"],
                    )
                    .pick_file()
                    .map(|path| crate::library_tools::inspect(&path))
            };
            if let Ok(mut state) = state.lock() {
                state.running = false;
                state.report = match result {
                    Some(Ok(report)) => report,
                    Some(Err(error)) => format!("Error: {error}"),
                    None => "Cancelled".into(),
                };
            }
        });
    }
}
