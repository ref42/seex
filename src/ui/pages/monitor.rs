//! The Monitor page: match modes, manual import, the export-file panel, the
//! action row, and the matched list.
//!
//! Ported from `SeexApp::monitor_page` in `src/main.rs` with the same metrics:
//! both panels sit at 267px, the matched list takes the height
//! `list_height(ui, 0.24, 3, 54.0)` used to compute against a 900px window, and
//! the columns are the same two-up split.

use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::*;

use crate::ui::app::{FileDialogKind, InputSlot, ManualImportStatus, SeexUi};
use crate::ui::{components, locale, theme};

const MONITOR_PANEL_HEIGHT: f32 = 267.0;
/// Five rows tall, the way the list reads best.
const MATCHED_LIST_HEIGHT: f32 = 270.0;
const BODY: f32 = 20.0;

fn v_space(height: f32) -> impl IntoElement {
    div().h(px(height))
}

impl SeexUi {
    pub(crate) fn monitor_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(self.match_modes(cx))
            .child(v_space(10.0))
            .child(self.import_and_export_row(window, cx))
            .child(v_space(10.0))
            .child(self.monitor_actions(cx))
            .child(v_space(10.0))
            .child(self.matched_list(cx))
            .into_any_element()
    }

    fn match_modes(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let monitoring = self
            .state
            .lock()
            .map(|state| state.monitoring)
            .unwrap_or(false);
        components::selectable(
            "monitoring-toggle",
            locale::t("Clipboard monitoring", "剪贴板监控"),
            monitoring,
            cx.listener(|this, _event: &ClickEvent, _window, cx| {
                if let Ok(mut state) = this.state.lock() {
                    let enabled = !state.monitoring;
                    state.set_monitoring(enabled);
                }
                cx.notify();
            }),
        )
        .into_any_element()
    }

    fn import_and_export_row(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .w_full()
            .gap(px(18.0))
            .child(div().flex_1().child(self.manual_import_panel(cx)))
            .child(div().flex_1().child(self.export_files_panel(window, cx)))
            .into_any_element()
    }

    fn manual_import_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let status = match &self.manual_import_status {
            Some(ManualImportStatus::Imported(count)) => Some(format!(
                "{} {}",
                count,
                locale::t("unique ID(s) imported.", "个唯一 ID 已导入。")
            )),
            Some(ManualImportStatus::NoIds) => {
                Some(locale::t("No LCSC component IDs found.", "未找到立创元件 ID。").to_string())
            }
            None => None,
        };

        // The button shares the title's line, so the box below gets the room the
        // action row used to take.
        // Fill the card: its 267px minus the 16px padding top and bottom, the
        // 40px title row and the gaps — less one line when a status shows.
        let box_height = if status.is_some() { 151.0 } else { 185.0 };

        let mut body = div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_size(px(BODY))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_color())
                            .child(locale::t("Manual import", "手动导入")),
                    )
                    .child(components::button(
                        "import-ids",
                        locale::t("Import IDs", "导入 ID"),
                        true,
                        cx.listener(|this, _event: &ClickEvent, window, cx| {
                            this.import_ids(window, cx);
                        }),
                    )),
            )
            .child(
                div()
                    .w_full()
                    .h(px(box_height))
                    .child(Textarea::new(&self.import_input).h(px(box_height))),
            );

        if let Some(status) = status {
            body = body.child(
                div()
                    .text_size(px(BODY))
                    .text_color(theme::muted())
                    .child(status),
            );
        }

        components::section_with_min_height(MONITOR_PANEL_HEIGHT, body).into_any_element()
    }

    fn export_files_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let bom_language = self
            .state
            .lock()
            .map(|state| state.bom_language.clone())
            .unwrap_or_default();
        let path_input = self.input(InputSlot::MatchedPath, window, cx);

        components::section_with_min_height(
            MONITOR_PANEL_HEIGHT,
            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(px(10.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_start()
                        .justify_between()
                        .w_full()
                        .gap(px(12.0))
                        .child(components::section_header(
                            locale::t("Export files", "导出文件"),
                            locale::t(
                                "UTF-8 CSV with metadata and capture counts. Quantity starts at 1.",
                                "UTF-8 CSV，含元数据和采集次数。数量默认为 1。",
                            ),
                        ))
                        .child(components::button(
                            "browse-save-csv",
                            locale::t("Browse", "浏览"),
                            true,
                            cx.listener(|this, _event: &ClickEvent, _window, cx| {
                                this.start_file_dialog(FileDialogKind::SaveCsv);
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(12.0))
                        .child(components::fixed_label(locale::t(
                            "BOM CSV file:",
                            "BOM CSV 文件:",
                        )))
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&path_input).text_size(px(20.0)).h(px(38.0))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(12.0))
                        .child(components::fixed_label(locale::t(
                            "Metadata language:",
                            "元数据语言:",
                        )))
                        .child(components::selectable(
                            "bom-chinese",
                            locale::t("Chinese", "中文"),
                            bom_language != "english",
                            cx.listener(|this, _event: &ClickEvent, _window, cx| {
                                if let Ok(mut state) = this.state.lock() {
                                    state.set_bom_language("chinese".to_string());
                                }
                                this.mark_config_dirty();
                                cx.notify();
                            }),
                        ))
                        .child(components::selectable(
                            "bom-english",
                            locale::t("English", "英文"),
                            bom_language == "english",
                            cx.listener(|this, _event: &ClickEvent, _window, cx| {
                                if let Ok(mut state) = this.state.lock() {
                                    state.set_bom_language("english".to_string());
                                }
                                this.mark_config_dirty();
                                cx.notify();
                            }),
                        )),
                ),
        )
        .into_any_element()
    }

    fn import_ids(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.import_input.read(cx).value().to_string();
        let count = self
            .state
            .lock()
            .map(|mut state| {
                let imported = crate::extract::extract_lcsc_ids(&text);
                let count = imported.len();
                state.import_text(text);
                count
            })
            .unwrap_or(0);
        self.manual_import_status = Some(if count == 0 {
            ManualImportStatus::NoIds
        } else {
            ManualImportStatus::Imported(count)
        });
        self.import_input
            .update(cx, |state, cx| state.set_value("", window, cx));
    }

    fn monitor_actions(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (matched_count, bom_running, bom_result) = self
            .state
            .lock()
            .map(|state| {
                (
                    state.matched.len(),
                    state.bom_running,
                    state.bom_last_result.clone(),
                )
            })
            .unwrap_or((0, false, None));

        let export_label = if bom_running {
            locale::t("Exporting...", "导出中...")
        } else {
            locale::t("Export BOM CSV", "导出 BOM CSV")
        };

        let clear = if self.clear_confirm {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .text_size(px(BODY))
                        .text_color(theme::danger())
                        .child(locale::t("Sure?", "确定吗?")),
                )
                .child(components::button(
                    "clear-yes",
                    locale::t("Yes", "是"),
                    true,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        if let Ok(mut state) = this.state.lock() {
                            state.clear_all();
                        }
                        this.clear_confirm = false;
                        this.show_toast_info(locale::t("Cleared all data", "已清空所有数据"));
                        cx.notify();
                    }),
                ))
                .child(components::button(
                    "clear-no",
                    locale::t("No", "否"),
                    true,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        this.clear_confirm = false;
                        cx.notify();
                    }),
                ))
        } else {
            div()
                .flex()
                .flex_row()
                .items_center()
                .child(components::button(
                    "clear-all",
                    locale::t("Clear All", "清空全部"),
                    true,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        this.clear_confirm = true;
                        cx.notify();
                    }),
                ))
        };

        let mut row = div().flex().flex_col().w_full().gap(px(10.0)).child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(12.0))
                .child(components::metric_pill(
                    locale::t("Matched", "匹配结果"),
                    matched_count,
                    theme::primary(),
                ))
                .child(components::button(
                    "copy-ids",
                    locale::t("Copy IDs", "复制 ID"),
                    true,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        this.copy_ids(cx);
                    }),
                ))
                .child(components::button(
                    "export-bom",
                    export_label,
                    matched_count > 0 && !bom_running,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        crate::bom::spawn_save_matched(std::sync::Arc::clone(&this.state));
                        cx.notify();
                    }),
                ))
                .child(clear),
        );

        if let Some(result) = bom_result {
            let color = if bom_running {
                theme::muted()
            } else {
                theme::success()
            };
            row = row.child(div().text_size(px(BODY)).text_color(color).child(result));
        }

        row.into_any_element()
    }

    fn copy_ids(&mut self, cx: &mut Context<Self>) {
        let ids = self
            .state
            .lock()
            .map(|state| state.get_unique_ids())
            .unwrap_or_default();
        if ids.is_empty() {
            self.show_toast_info(locale::t("No IDs to copy", "没有可复制的 ID"));
        } else {
            crate::ui::app::copy_to_clipboard(&ids.join("\n"));
            let count = ids.len();
            let message = match locale::lang() {
                crate::ui::locale::Lang::En => format!("Copied {count} IDs"),
                crate::ui::locale::Lang::Zh => format!("已复制 {count} 个 ID"),
            };
            self.show_toast_success(message);
        }
        cx.notify();
    }

    fn matched_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let items = self
            .state
            .lock()
            .map(|state| state.matched.clone())
            .unwrap_or_default();

        let mut body = div().flex().flex_col().w_full().gap(px(10.0)).child(
            components::section_header_inline(
                locale::t("Matched", "匹配结果"),
                components::count_chip(items.len(), theme::primary()),
            ),
        );

        if items.is_empty() {
            body = body.child(components::empty_state(locale::t(
                "No matches yet",
                "暂无匹配结果",
            )));
        } else {
            let mut list = div()
                .id("matched-list")
                .flex()
                .flex_col()
                .w_full()
                .h(px(MATCHED_LIST_HEIGHT))
                .overflow_y_scroll();
            for (index, item) in items.iter().enumerate() {
                list = list.child(
                    div().mb(px(6.0)).child(components::row_frame(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .w_full()
                            .gap(px(12.0))
                            .child(
                                div()
                                    .text_size(px(BODY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::primary())
                                    .child(item.lcsc_id.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(components::small_button(
                                        format!("copy-matched-{index}"),
                                        locale::t("Copy", "复制"),
                                        {
                                            let id = item.lcsc_id.clone();
                                            cx.listener(
                                                move |this, _event: &ClickEvent, _window, cx| {
                                                    crate::ui::app::copy_to_clipboard(&id);
                                                    this.show_toast_success(locale::t(
                                                        "Copied",
                                                        "已复制",
                                                    ));
                                                    cx.notify();
                                                },
                                            )
                                        },
                                    ))
                                    .child(components::small_button(
                                        format!("delete-matched-{index}"),
                                        locale::t("Delete", "删除"),
                                        cx.listener(
                                            move |this, _event: &ClickEvent, _window, cx| {
                                                if let Ok(mut state) = this.state.lock() {
                                                    state.delete_matched(index);
                                                }
                                                cx.notify();
                                            },
                                        ),
                                    )),
                            ),
                    )),
                );
            }
            body = body.child(list);
        }

        components::section(body).into_any_element()
    }
}
