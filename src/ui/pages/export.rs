//! The Export page: the export run at the top, the settings that feed it
//! underneath.
//!
//! Ported from `SeexApp::export_page` in `src/main.rs`. The two panels, the
//! 180px label column, the scope buttons, and the toggle row are the same; the
//! failed-ID list, which used to pop up over the page, is now shown inside the
//! export panel where it cannot cover anything.

use gpui_kit::component::input::Input;
use gpui_kit::*;

use crate::ui::app::{ColorSlot, FileDialogKind, HintArea, InputSlot, SeexUi};
use crate::ui::locale::Lang;
use crate::ui::{components, locale, theme};

const BODY: f32 = 20.0;
const SMALL: f32 = 14.0;

/// The five numbers `render_export_metrics` scraped out of an export result.
fn metric_values(result: &str) -> Vec<(&'static str, String)> {
    let mut metrics: Vec<(&'static str, String)> = Vec::new();
    for line in result.lines().filter(|line| !line.trim().is_empty()) {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Elapsed:") {
            metrics.push(("Elapsed", value.trim().to_string()));
        } else if trimmed.contains("Total:") {
            for key in ["Total", "Success", "Skipped", "Failed"] {
                if let Some((_, value)) = trimmed.split_once(&format!("{key}:")) {
                    metrics.push((
                        key,
                        value.split('|').next().unwrap_or(value).trim().to_string(),
                    ));
                }
            }
        }
    }
    metrics
}

fn metric_labels() -> [&'static str; 5] {
    match locale::lang() {
        Lang::En => ["Elapsed", "Total", "Success", "Skipped", "Failed"],
        Lang::Zh => ["耗时", "总数", "成功", "跳过", "失败"],
    }
}

fn failed_ids(result: &str) -> Vec<String> {
    result
        .lines()
        .find(|line| line.trim().to_ascii_lowercase().starts_with("failed ids:"))
        .map(|line| crate::extract::extract_lcsc_ids(line))
        .unwrap_or_default()
}

/// The reason a run failed, for the page to show.
///
/// A failed run reports no counts, so without this a failure that never got as far as
/// exporting anything looked like a run that did nothing at all.
fn failure_reason(result: &str) -> Option<String> {
    let mut lines = result.lines().map(str::trim).filter(|l| !l.is_empty());
    let status = lines.clone().find(|line| line.starts_with("Status:"))?;
    if !status.to_ascii_lowercase().contains("failed") {
        return None;
    }
    lines
        .find(|line| {
            ![
                "Status:",
                "Target:",
                "Elapsed:",
                "Failed IDs:",
                "Failed ids:",
                "Targets:",
                "Output directory:",
                "Library name:",
                "Parallel:",
                "Continue on error:",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
        })
        .map(ToOwned::to_owned)
}

/// The parts whose exported 3D model still carries the EasyEDA logo.
fn models_with_logo(result: &str) -> Option<String> {
    result
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix(crate::step_model::LOGO_KEPT_PREFIX)
        })
        .map(|parts| parts.trim().to_string())
        .filter(|parts| !parts.is_empty())
}

impl SeexUi {
    pub(crate) fn export_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            // The two panels above keep their own height; the failure list below
            // is the one that gives way when the window is short.
            .child(div().flex_shrink_0().child(self.export_run_panel(cx)))
            .child(div().h(px(10.0)))
            .child(
                div()
                    .flex_shrink_0()
                    .child(self.export_settings_panel(window, cx)),
            )
            .child(div().h(px(10.0)))
            .child(self.failed_ids_panel(cx))
            .into_any_element()
    }

    /// The failed components of the last run, given the bottom of the page.
    ///
    /// They used to be one comma-joined line inside the run panel, which cut
    /// them off as soon as there were a few; here each id is a chip of its own
    /// and the list scrolls rather than clipping.
    fn failed_ids_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ids = self.last_failed_ids();
        let count = ids.len();
        let accent = if count > 0 {
            theme::danger()
        } else {
            theme::muted()
        };

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .w_full()
            .child(
                div()
                    .text_size(px(BODY))
                    .text_color(theme::text_color())
                    .child(locale::t("Failed IDs", "失败 ID")),
            )
            .child(components::count_chip(count, accent))
            .child(div().flex_1())
            .child(components::button(
                "copy-failed-ids",
                locale::t("Copy IDs", "复制 ID"),
                count > 0,
                cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    this.copy_failed_ids(cx);
                }),
            ));

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .w_full()
            .child(header)
            .child(
                div()
                    .text_size(px(SMALL))
                    .text_color(theme::muted())
                    .child(locale::t(
                        "Components the last export could not write.",
                        "上次导出未能写入的元件。",
                    )),
            );

        if ids.is_empty() {
            body = body.child(components::empty_state(locale::t(
                "Nothing failed in the last export.",
                "上次导出没有失败项。",
            )));
        } else {
            let mut chips = div().flex().flex_row().flex_wrap().gap(px(8.0)).w_full();
            for id in &ids {
                chips = chips.child(components::chip(id.clone(), theme::danger()));
            }
            body = body.child(
                div()
                    .id("failed-ids-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .w_full()
                    .overflow_y_scroll()
                    .child(chips),
            );
        }

        components::section_fill(body).into_any_element()
    }

    /// The ids the last export failed on.
    fn last_failed_ids(&self) -> Vec<String> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.export_last_result.clone())
            .map(|result| failed_ids(&result))
            .unwrap_or_default()
    }

    /// Copies the failed ids, the way the Monitor page copies the matched ones.
    fn copy_failed_ids(&mut self, cx: &mut Context<Self>) {
        let ids = self.last_failed_ids();
        if ids.is_empty() {
            self.show_toast_info(locale::t("No failed IDs to copy", "没有可复制的失败 ID"));
        } else {
            crate::ui::app::copy_to_clipboard(&ids.join("\n"));
            let count = ids.len();
            let message = match locale::lang() {
                Lang::En => format!("Copied {count} failed IDs"),
                Lang::Zh => format!("已复制 {count} 个失败 ID"),
            };
            self.show_toast_success(message);
        }
        cx.notify();
    }

    fn export_run_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (matched_count, running, result, progress) = self
            .state
            .lock()
            .map(|state| {
                (
                    state.matched.len(),
                    state.export_running,
                    state.export_last_result.clone(),
                    state.export_progress.as_ref().map(|progress| {
                        (progress.current, progress.total, progress.message.clone())
                    }),
                )
            })
            .unwrap_or((0, false, None, None));

        let mut body = div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(10.0))
            .child(components::section_header_inline(
                locale::t("EDA Library Export", "EDA 库导出"),
                components::metric_pill(
                    locale::t("Ready", "待导出"),
                    matched_count,
                    theme::primary(),
                ),
            ))
            .child(
                div()
                    .text_size(px(BODY))
                    .text_color(theme::muted())
                    .child(locale::t(
                        "Altium, KiCad and Cadence export",
                        "导出 Altium、KiCad 和 Cadence 库",
                    )),
            );

        let mut row = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .w_full()
            .gap(px(12.0));

        if let Some(result) = &result {
            let metrics = metric_values(result);
            for (index, label) in metric_labels().iter().enumerate() {
                let key = ["Elapsed", "Total", "Success", "Skipped", "Failed"][index];
                let value = metrics
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| value.clone())
                    .unwrap_or_else(|| "0".to_string());
                let color = if key == "Elapsed" || key == "Success" {
                    theme::success()
                } else if key == "Failed" && value != "0" {
                    theme::danger()
                } else {
                    theme::text_color()
                };
                // A plain count now: the ids themselves have their own panel at
                // the bottom of the page.
                row = row.child(components::pill_text(format!("{label} {value}"), color));
            }
        }

        // The bar takes every pixel the button does not need, which is the
        // width the egui page gave it (`available_width - 160`). A spacer next
        // to it would split that space in two and leave the bar half-length.
        let bar = progress.map(|(current, total, message)| {
            let value = if total > 0 {
                current as f32 / total as f32
            } else {
                0.0
            };
            components::progress_bar(value, message)
        });
        if let Some(bar) = bar {
            row = row.child(bar);
        } else {
            row = row.child(div().flex_1());
        }

        row = row.child(components::button(
            "export-library",
            if running {
                locale::t("Running...", "运行中...")
            } else {
                locale::t("Export Library", "导出库")
            },
            matched_count > 0 && !running,
            cx.listener(|this, _event: &ClickEvent, _window, cx| {
                this.start_export();
                cx.notify();
            }),
        ));

        body = body.child(row);

        if let Some(reason) = result.as_deref().and_then(failure_reason) {
            body = body.child(
                div()
                    .w_full()
                    .text_size(px(BODY))
                    .text_color(theme::danger())
                    .child(reason),
            );
        }
        if let Some(parts) = result.as_deref().and_then(models_with_logo) {
            let message = match locale::lang() {
                Lang::En => format!(
                    "The EasyEDA logo could not be removed from these 3D models; the original models were exported: {parts}"
                ),
                Lang::Zh => {
                    format!("以下元件的 3D 模型无法去除 EasyEDA 标志，已导出原始模型：{parts}")
                }
            };
            body = body.child(
                div()
                    .w_full()
                    .text_size(px(BODY))
                    .text_color(theme::danger())
                    .child(message),
            );
        }

        components::section(body).into_any_element()
    }

    /// Builds the request the egui page built inline and hands it to the same
    /// exporter task.
    fn start_export(&mut self) {
        let request = {
            let Ok(state) = self.state.lock() else {
                return;
            };
            crate::exporter::ExportRequest {
                ids: state.get_unique_ids(),
                target: state.export_target.clone(),
                output_path: state.export_output_path.clone(),
                mode: match state.export_target.as_str() {
                    "kicad" => state.export_kicad_mode.clone(),
                    "cadence" => state.export_cadence_mode.clone(),
                    _ => state.export_mode.clone(),
                },
                merge: state.export_merge,
                append: state.export_append,
                library_name: state.export_library_name.clone(),
                parallel: state.export_parallel,
                continue_on_error: state.export_continue_on_error,
                lcsc_english: state.export_lcsc_english,
                force: state.export_force,
                schematic_stroke_color: self.schematic_stroke_color,
                schematic_fill_color: self.schematic_fill_color,
                remove_model_logo: state.export_remove_model_logo,
            }
        };
        crate::exporter::spawn_export(std::sync::Arc::clone(&self.state), request);
    }

    fn export_settings_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (target, mode, merge, append, continue_on_error, force, english, remove_logo) = self
            .state
            .lock()
            .map(|state| {
                (
                    state.export_target.clone(),
                    match state.export_target.as_str() {
                        "kicad" => state.export_kicad_mode.clone(),
                        "cadence" => state.export_cadence_mode.clone(),
                        _ => state.export_mode.clone(),
                    },
                    state.export_merge,
                    state.export_append,
                    state.export_continue_on_error,
                    state.export_force,
                    state.export_lcsc_english,
                    state.export_remove_model_logo,
                )
            })
            .unwrap_or_default();

        let path_input = self.input(InputSlot::ExportPath, window, cx);
        let library_input = self.input(InputSlot::LibraryName, window, cx);
        let parallel_input = self.input(InputSlot::Parallel, window, cx);
        let stroke_input = self.input(InputSlot::StrokeHex, window, cx);
        let fill_input = self.input(InputSlot::FillHex, window, cx);
        // The pickers behind the two swatches. A colour typed into a hex box is
        // folded in here, where the `Window` the sliders need is available.
        let stroke_picker = self.color_picker(ColorSlot::Stroke, window, cx);
        let fill_picker = self.color_picker(ColorSlot::Fill, window, cx);
        self.sync_color_pickers(window, cx);
        stroke_picker.update(cx, |state, cx| state.sync_pending_value(window, cx));
        fill_picker.update(cx, |state, cx| state.sync_pending_value(window, cx));

        let target_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .child(components::fixed_label(locale::t(
                "EDA target:",
                "EDA 目标:",
            )))
            .child(components::selectable(
                "target-altium",
                "Altium",
                target != "kicad" && target != "cadence",
                cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    if let Ok(mut state) = this.state.lock() {
                        state.set_export_target("altium".to_string());
                    }
                    this.close_color_pickers(cx);
                    this.mark_config_dirty();
                    cx.notify();
                }),
            ))
            .child(components::selectable(
                "target-kicad",
                "KiCad",
                target == "kicad",
                cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    if let Ok(mut state) = this.state.lock() {
                        state.set_export_target("kicad".to_string());
                    }
                    this.close_color_pickers(cx);
                    this.mark_config_dirty();
                    cx.notify();
                }),
            ))
            .child(components::selectable(
                "target-cadence",
                "Cadence",
                target == "cadence",
                cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    if let Ok(mut state) = this.state.lock() {
                        state.set_export_target("cadence".to_string());
                    }
                    this.close_color_pickers(cx);
                    this.mark_config_dirty();
                    cx.notify();
                }),
            ));

        let mut scope_row =
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .child(components::fixed_label(locale::t(
                    "Export scope:",
                    "导出范围:",
                )));

        let modes: [(&'static str, &'static str); 4] = match target.as_str() {
            "kicad" => [
                ("full", locale::t("Full", "完整")),
                ("symbol", locale::t("Symbol", "符号")),
                ("footprint", locale::t("Footprint", "封装")),
                ("3d", "3D"),
            ],
            "cadence" => [
                ("full", locale::t("Full", "完整")),
                ("symbol", "Capture"),
                ("footprint", "Allegro"),
                ("3d", "3D"),
            ],
            _ => [
                ("full", locale::t("Full", "完整")),
                ("schlib", "SchLib"),
                ("pcblib", "PcbLib"),
                ("3d", "3D"),
            ],
        };
        for (value, label) in modes {
            let target = target.clone();
            scope_row = scope_row.child(components::selectable(
                format!("scope-{value}"),
                label,
                mode == value,
                cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                    if let Ok(mut state) = this.state.lock() {
                        match target.as_str() {
                            "kicad" => state.set_export_kicad_mode(value.to_string()),
                            "cadence" => state.set_export_cadence_mode(value.to_string()),
                            _ => state.set_export_mode(value.to_string()),
                        }
                    }
                    this.mark_config_dirty();
                    cx.notify();
                }),
            ));
        }

        let mut toggles = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .flex_1()
            .gap(px(12.0));

        // Merge belongs to Altium alone: a KiCad or Cadence export always writes
        // one named library, so there is nothing to merge. Append applies to
        // all three, with the same meaning in each. Altium's 3D scope only
        // downloads models, so neither applies there.
        let altium = target != "kicad" && target != "cadence";
        if !(altium && mode == "3d") {
            if altium {
                toggles = toggles.child(components::hint(
                    components::selectable(
                        "toggle-merge",
                        locale::t("Merge", "合并"),
                        merge,
                        cx.listener(|this, _event: &ClickEvent, _window, cx| {
                            if let Ok(mut state) = this.state.lock() {
                                let next = !state.export_merge;
                                state.set_export_merge(next);
                            }
                            this.mark_config_dirty();
                            cx.notify();
                        }),
                    ),
                    locale::t(
                        "Merge: export all components into one library file",
                        "合并：所有元件输出到同一个库文件",
                    ),
                    HintArea::ExportToggles,
                    cx,
                ));
            }
            toggles = toggles.child(components::hint(
                components::selectable(
                    "toggle-append",
                    locale::t("Append", "追加"),
                    append,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        if let Ok(mut state) = this.state.lock() {
                            let next = !state.export_append;
                            state.set_export_append(next);
                        }
                        this.mark_config_dirty();
                        cx.notify();
                    }),
                ),
                locale::t(
                    "Append: add to an existing library instead of overwriting",
                    "追加：向已有库文件追加，而不是覆盖",
                ),
                HintArea::ExportToggles,
                cx,
            ));
        }

        toggles = toggles
            .child(components::hint(
                components::selectable(
                    "toggle-continue",
                    locale::t("Continue On Error", "出错继续"),
                    continue_on_error,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        if let Ok(mut state) = this.state.lock() {
                            let next = !state.export_continue_on_error;
                            state.set_export_continue_on_error(next);
                        }
                        this.mark_config_dirty();
                        cx.notify();
                    }),
                ),
                locale::t(
                    "Continue On Error: skip failed components, keep exporting",
                    "出错继续：跳过失败的元件，不中断导出",
                ),
                HintArea::ExportToggles,
                cx,
            ))
            .child(components::hint(
                components::selectable(
                    "toggle-force",
                    locale::t("Force", "强制"),
                    force,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        if let Ok(mut state) = this.state.lock() {
                            let next = !state.export_force;
                            state.set_export_force(next);
                        }
                        this.mark_config_dirty();
                        cx.notify();
                    }),
                ),
                locale::t(
                    "Force: overwrite components that already exist",
                    "强制：覆盖库中已存在的元件",
                ),
                HintArea::ExportToggles,
                cx,
            ))
            .child(components::hint(
                components::selectable(
                    "toggle-english",
                    locale::t("English metadata", "英文元数据"),
                    english,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        if let Ok(mut state) = this.state.lock() {
                            let english = !state.export_lcsc_english;
                            state.set_export_lcsc_english(english);
                            state.bom_language =
                                if english { "english" } else { "chinese" }.to_string();
                        }
                        this.mark_config_dirty();
                        cx.notify();
                    }),
                ),
                locale::t(
                    "English metadata: write component fields in English",
                    "英文元数据：元件属性字段使用英文",
                ),
                HintArea::ExportToggles,
                cx,
            ))
            .child(components::hint(
                components::selectable(
                    "toggle-remove-logo",
                    locale::t("Remove 3D logo", "去除 3D 标志"),
                    remove_logo,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        if let Ok(mut state) = this.state.lock() {
                            let next = !state.export_remove_model_logo;
                            state.set_export_remove_model_logo(next);
                        }
                        this.mark_config_dirty();
                        cx.notify();
                    }),
                ),
                locale::t(
                    "Remove 3D logo: rebuild 3D models without the EasyEDA logo; a model it cannot be removed from is exported as it is and listed",
                    "去除 3D 标志：重建 3D 模型并去除 EasyEDA 标志；无法去除的模型按原样导出并列出",
                ),
                HintArea::ExportToggles,
                cx,
            ));

        let mut body = div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(10.0))
            .child(components::section_header(
                locale::t("Export Settings", "导出设置"),
                locale::t("Defaults are saved automatically.", "设置会自动保存。"),
            ))
            .child(target_row);

        if target != "kicad" && target != "cadence" {
            // The fill can be "no colour", so its palette leads with the
            // candidate for that; the stroke always has a colour.
            let no_fill = components::none_color_candidate(
                "sch-fill-none",
                self.schematic_fill_color.is_none(),
                cx.listener(|this, _event: &ClickEvent, window, cx| {
                    this.clear_color(ColorSlot::Fill, window, cx);
                }),
            );
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.0))
                    .child(components::color_setting(
                        "sch-stroke-color",
                        locale::t("SCH stroke color:", "原理图描边颜色:"),
                        self.color_of(ColorSlot::Stroke),
                        &stroke_picker,
                        &stroke_input,
                        None,
                        cx,
                    ))
                    .child(components::color_setting(
                        "sch-fill-color",
                        locale::t("SCH fill color:", "原理图填充颜色:"),
                        self.color_of(ColorSlot::Fill),
                        &fill_picker,
                        &fill_input,
                        Some(no_fill),
                        cx,
                    )),
            );
        }

        body = body
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .child(components::fixed_label(locale::t(
                        "Export directory:",
                        "导出目录:",
                    )))
                    .child(div().flex_1().child(Input::new(&path_input).h(px(30.0))))
                    .child(components::button(
                        "browse-export-dir",
                        locale::t("Browse", "浏览"),
                        true,
                        cx.listener(|this, _event: &ClickEvent, _window, cx| {
                            this.start_file_dialog(FileDialogKind::PickFolder);
                            cx.notify();
                        }),
                    )),
            )
            .child(scope_row)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .child(components::fixed_label(locale::t(
                        "Library name:",
                        "库名称:",
                    )))
                    .child(
                        div()
                            .w(px(320.0))
                            .child(Input::new(&library_input).h(px(30.0))),
                    )
                    .child(
                        div()
                            .text_size(px(BODY))
                            .text_color(theme::muted())
                            .child(locale::t("Parallel jobs:", "并行任务数:")),
                    )
                    .child(
                        div()
                            .w(px(80.0))
                            .child(Input::new(&parallel_input).h(px(30.0))),
                    ),
            )
            // These options have no label of their own, so they lead with the
            // same gutter the labelled rows above use. Keeping the gutter
            // outside the wrapping row lines every wrapped line up with `Full`
            // and `SchLib`, rather than only the first.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .w_full()
                    .gap(px(12.0))
                    .child(components::label_gutter())
                    .child(toggles),
            );

        // The explanation for a toggle sits directly under the row, where it is
        // next to the button it describes and has room to be read in full.
        //
        // The slot is always here, empty or not. Adding and removing the box
        // itself moved everything below it under the pointer, which took the
        // pointer off the button, which removed the box again: the hint and the
        // layout flickered for as long as the pointer rested on a toggle.
        let hint = match self.hover_hint.clone() {
            Some((HintArea::ExportToggles, hint)) => {
                components::status_box(hint, theme::primary()).into_any_element()
            }
            _ => div().into_any_element(),
        };
        body = body.child(div().h(px(34.0)).w_full().child(hint));

        components::section(body).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::failure_reason;

    #[test]
    fn names_the_reason_a_run_failed() {
        let failed = "Status: Failed\nTarget: Altium\nElapsed: 0.01s\n\
            io error: Access is denied. (os error 5)\nTargets: Full\n\
            Output directory: C:/libs\nLibrary name: conn";
        assert_eq!(
            failure_reason(failed).as_deref(),
            Some("io error: Access is denied. (os error 5)")
        );
    }

    #[test]
    fn says_nothing_about_a_run_that_worked() {
        let ok = "Status: Success\nTarget: Altium\nElapsed: 1.00s\n\
            export Altium batch complete. Total: 1 | Skipped: 0 | Success: 1 | Failed: 0";
        assert_eq!(failure_reason(ok), None);
    }
}
