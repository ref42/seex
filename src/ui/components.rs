//! Reusable pieces of the SeEx chrome, moved out of `main.rs` so both front ends
//! read the same metrics.
//!
//! Everything here mirrors the egui originals: the body/button face is
//! `scaled_font(26.0)` = 19.5px, buttons are 40px tall with 16×8 padding and a
//! 2px radius on `field_bg`, panels are square-cornered `surface` with a 1px
//! `border` and 18×16 padding, and rows are `row_bg` with a `subtle_border`.

use gpui_kit::*;
// The swatch element lives in the unstyled base layer, along with the
// `Selectable` trait `Popover` asks of its trigger.
use gpui_kit::base::{ColorSwatch, Selectable};
use gpui_kit::component::Colorize as _;
use gpui_kit::component::color_picker::ColorPickerState;
use gpui_kit::component::input::InputState;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::slider::{Slider, SliderState};

use crate::ui::theme;

const BODY: f32 = 20.0;
const SMALL: f32 = 14.0;
/// Width of the label column that lines up the fields in a form.
const LABEL_COLUMN_WIDTH: f32 = 180.0;

/// A filled button in the egui style: 40px tall, `field_bg`, 1px border.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let element = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(40.0))
        .px(px(16.0))
        .rounded(px(theme::RADIUS_SM))
        .bg(theme::field_bg())
        .border(px(1.0))
        .border_color(theme::border())
        .text_size(px(BODY))
        .text_color(theme::text_color())
        .cursor_pointer()
        .child(label.into());

    if enabled {
        element
            .hover(|style| style.bg(theme::widget_hover_bg()))
            .on_click(on_click)
    } else {
        element.opacity(0.5)
    }
}

/// A small button for list rows.
pub fn small_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(28.0))
        .px(px(10.0))
        .rounded(px(theme::RADIUS_SM))
        .bg(theme::field_bg())
        .border(px(1.0))
        .border_color(theme::border())
        .text_size(px(SMALL))
        .text_color(theme::text_color())
        .cursor_pointer()
        .hover(|style| style.bg(theme::widget_hover_bg()))
        .child(label.into())
        .on_click(on_click)
}

/// `selectable_label`: the primary colour fills the button while it is chosen.
pub fn selectable(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let (background, text) = if selected {
        (theme::primary(), theme::white())
    } else {
        (theme::field_bg(), theme::text_color())
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(40.0))
        .px(px(16.0))
        .rounded(px(theme::RADIUS_SM))
        .bg(background)
        .border(px(1.0))
        .border_color(theme::border())
        .text_size(px(BODY))
        .text_color(text)
        .cursor_pointer()
        .hover(move |style| {
            if selected {
                style
            } else {
                style.bg(theme::widget_hover_bg())
            }
        })
        .child(label.into())
        .on_click(on_click)
}

/// A button that fills its share of a row, for the theme-preset grid.
pub fn theme_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let (background, text) = if selected {
        (theme::primary(), theme::white())
    } else {
        (theme::field_bg(), theme::text_color())
    };
    div()
        .id(id)
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .h(px(38.0))
        .rounded(px(theme::RADIUS_SM))
        .bg(background)
        .border(px(1.0))
        .border_color(theme::border())
        .text_size(px(18.0))
        .text_color(text)
        .cursor_pointer()
        .hover(move |style| {
            if selected {
                style
            } else {
                style.bg(theme::widget_hover_bg())
            }
        })
        .child(label.into())
        .on_click(on_click)
}

/// A top-bar button: `width` wide, 44px tall, primary-filled when selected.
pub fn top_bar_button(
    id: impl Into<ElementId>,
    width: f32,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let text_color = if selected {
        theme::white()
    } else {
        theme::text_color()
    };
    let background = if selected {
        theme::primary()
    } else {
        theme::top_bar_button_bg()
    };

    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(width))
        .px(px(16.0))
        .flex_shrink_0()
        .whitespace_nowrap()
        .h(px(44.0))
        .rounded(px(theme::RADIUS_MD))
        .bg(background)
        .hover(move |style| {
            if selected {
                style
            } else {
                style.bg(theme::top_bar_hover())
            }
        })
        .cursor_pointer()
        .text_size(px(theme::scaled_font(24.0)))
        .text_color(text_color)
        .child(label.into())
        .on_click(on_click)
}

/// A panel: `surface` fill, 1px border, 18×16 padding, 10px between children.
pub fn section(body: impl IntoElement) -> impl IntoElement {
    panel(0.0, body)
}

/// The same panel pinned to the height the neighbouring column settles at.
pub fn section_with_min_height(min_height: f32, body: impl IntoElement) -> impl IntoElement {
    panel(min_height, body)
}

/// The same panel stretched to fill what is left of a column, for the one panel
/// on a page whose content grows — the Export page keeps its failed ids in one
/// and the History page its log.
///
/// `min_h(0)` matters: a flex item will not shrink below its content by default,
/// so without it a long list pushed the panel past the bottom of the window and
/// the panel's own outline was clipped instead of the list scrolling inside it.
pub fn section_fill(body: impl IntoElement) -> Div {
    panel(0.0, body).flex_1().min_h(px(0.0))
}

fn panel(min_height: f32, body: impl IntoElement) -> Div {
    let element = div()
        .flex()
        .flex_col()
        .w_full()
        .px(px(18.0))
        .py(px(16.0))
        .gap(px(10.0))
        .bg(theme::surface())
        .border(px(1.0))
        .border_color(theme::border())
        .rounded(px(theme::RADIUS_LG))
        .child(body);

    if min_height > 0.0 {
        // egui reserved an exact size here, so children that grow have a
        // definite box to grow into.
        element.h(px(min_height)).min_h(px(min_height))
    } else {
        element
    }
}

/// A list row: `row_bg`, 1px `subtle_border`, 12×8 padding.
pub fn row_frame(body: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .w_full()
        .px(px(12.0))
        .py(px(8.0))
        .bg(theme::row_bg())
        .border(px(1.0))
        .border_color(theme::subtle_border())
        .rounded(px(theme::RADIUS_SM))
        .child(body)
}

/// A panel title with its one-line description underneath.
pub fn section_header(
    title: impl Into<SharedString>,
    desc: impl Into<SharedString>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(BODY))
                .text_color(theme::text_color())
                .child(title.into()),
        )
        .child(
            div()
                .text_size(px(SMALL))
                .text_color(theme::muted())
                .child(desc.into()),
        )
}

/// A title with room for a control on the right-hand edge.
pub fn section_header_inline(
    title: impl Into<SharedString>,
    right: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .w_full()
        .child(
            div()
                .text_size(px(BODY))
                .text_color(theme::text_color())
                .child(title.into()),
        )
        .child(right)
}

/// The 180×30 label column that lines up the fields in a form.
pub fn fixed_label(label: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .w(px(LABEL_COLUMN_WIDTH))
        .h(px(30.0))
        .text_size(px(BODY))
        .text_color(theme::muted())
        .child(label.into())
}

/// The empty column a row without its own label leads with, so that its controls
/// line up with the ones in the labelled rows above it.
pub fn label_gutter() -> impl IntoElement {
    div().w(px(LABEL_COLUMN_WIDTH)).flex_shrink_0()
}

pub fn empty_state(label: impl Into<SharedString>) -> impl IntoElement {
    div()
        .mt(px(8.0))
        .text_size(px(BODY))
        .text_color(theme::muted())
        .italic()
        .child(label.into())
}

/// A rounded counter, sized from its text like the painted egui chip.
pub fn chip(label: impl Into<SharedString>, color: Rgba) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .px(px(9.0))
        .py(px(4.0))
        .rounded(px(12.0))
        .bg(theme::chip_bg(color))
        .text_size(px(theme::scaled_font(20.0)))
        .text_color(color)
        .child(label.into())
}

pub fn count_chip(value: usize, color: Rgba) -> impl IntoElement {
    chip(value.to_string(), color)
}

/// One half of the Font page: the slot's name, the family in use, and the
/// button that opens its picker.
pub fn font_slot_row(
    id: &'static str,
    label: impl Into<SharedString>,
    family: impl Into<SharedString>,
    open: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let marker = if open { "\u{25B2}" } else { "\u{25BC}" };
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .gap(px(12.0))
        .child(
            div()
                .w(px(64.0))
                .text_size(px(BODY))
                .text_color(theme::muted())
                .child(label.into()),
        )
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(BODY))
                .text_color(theme::text_color())
                .child(family.into()),
        )
        .child(selectable(format!("{id}-toggle"), marker, open, on_click))
}

/// Attaches a hover hint. Rather than a floating tooltip — which landed on top
/// of the buttons around it — the text is parked on the view and shown in the
/// status bar, where it cannot cover anything.
pub fn hint(
    element: Stateful<Div>,
    text: impl Into<SharedString>,
    area: crate::ui::app::HintArea,
    cx: &mut Context<crate::ui::app::SeexUi>,
) -> Stateful<Div> {
    let text: SharedString = text.into();
    element.on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
        if *hovered {
            this.hover_hint = Some((area, text.clone()));
        } else if matches!(&this.hover_hint, Some((_, current)) if current == &text) {
            // Only clear our own hint: the pointer may already be on the next
            // button, which has set its own by now.
            this.hover_hint = None;
        }
        cx.notify();
    }))
}

/// A single family in the picker list.
pub fn font_row(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let name: SharedString = name.into();
    let text = if selected {
        theme::white()
    } else {
        theme::text_color()
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .px(px(12.0))
        .py(px(6.0))
        .bg(if selected {
            theme::primary()
        } else {
            theme::row_bg()
        })
        .border(px(1.0))
        .border_color(theme::subtle_border())
        .rounded(px(theme::RADIUS_SM))
        .cursor_pointer()
        .hover(move |style| {
            if selected {
                style
            } else {
                style.bg(theme::widget_hover_bg())
            }
        })
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(BODY))
                .text_color(text)
                .child(name.clone()),
        )
        .child(
            div()
                .flex_shrink_0()
                .font_family(name)
                .text_size(px(BODY))
                .text_color(text)
                .child("Aa 汉字"),
        )
        .on_click(on_click)
}

/// The single style for everything the status bar has to say — a running notice
/// or a hover hint — so the two read as the same kind of thing: a filled box in
/// the accent colour, with white text.
pub fn status_box(text: impl Into<SharedString>, accent: Rgba) -> impl IntoElement {
    status_box_shape(accent).child(text.into())
}

/// The same box as a button, for the one thing in the bar that is worth
/// clicking: it looks identical, and lifts a little under the pointer so the
/// click is discoverable.
pub fn status_button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    accent: Rgba,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let hover: Hsla = accent.into();
    status_box_shape(accent)
        .id(id)
        .cursor_pointer()
        .hover(move |style| style.bg(hover.lighten(0.12)))
        .on_click(on_click)
        .child(text.into())
}

/// The box both of the above are built from.
fn status_box_shape(accent: Rgba) -> Div {
    div()
        .flex()
        .items_center()
        .h(px(34.0))
        .px(px(12.0))
        .rounded(px(6.0))
        .bg(accent)
        .border(px(1.0))
        .border_color(accent)
        .overflow_hidden()
        .text_ellipsis()
        .text_size(px(theme::scaled_font(23.0)))
        .text_color(theme::white())
}

/// A wide, count-bearing pill that heads an action row.
pub fn metric_pill(label: impl Into<SharedString>, value: usize, color: Rgba) -> impl IntoElement {
    let label: SharedString = label.into();
    div()
        .flex()
        .items_center()
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(12.0))
        .bg(theme::chip_bg(color))
        .text_size(px(theme::scaled_font(25.0)))
        .text_color(color)
        .child(format!("{label} {value}"))
}

/// The same pill with its text already assembled, for the export metrics.
pub fn pill_text(text: impl Into<SharedString>, color: Rgba) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(12.0))
        .bg(theme::chip_bg(color))
        .text_size(px(theme::scaled_font(25.0)))
        .text_color(color)
        .child(text.into())
}

/// A progress bar in the theme's primary colour, with its message written
/// inside the track the way egui drew it.
pub fn progress_bar(value: f32, message: impl Into<SharedString>) -> impl IntoElement {
    let value = value.clamp(0.0, 1.0);
    let message: SharedString = message.into();
    div()
        .relative()
        .flex_1()
        .min_w(px(200.0))
        .h(px(34.0))
        .rounded(px(theme::RADIUS_SM))
        .bg(theme::field_bg())
        .border(px(1.0))
        .border_color(theme::border())
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .h_full()
                .w(relative(value))
                .bg(theme::progress_fill()),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w_full()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(theme::scaled_font(22.0)))
                .text_color(theme::text_color())
                .child(message),
        )
}

/// `color_setting`: the 180px label column, the round swatch that opens the
/// colour picker, and the `#RRGGBB` box that keeps the swatch in step with what
/// is typed.
///
/// The picker is the kit's [`ColorPickerState`]: it owns the palette, the H/S/L
/// sliders and the commit events, so the page only has to render the trigger
/// and hand the chosen colour back through that state's change event. A `None`
/// colour is the "no colour" choice: the swatch shows the slash symbol, and
/// `none` — the candidate for it in the palette — is drawn by the page, which
/// owns what "no colour" means for that slot.
pub fn color_setting(
    id: &'static str,
    label: impl Into<SharedString>,
    color: Option<[u8; 3]>,
    picker: &Entity<ColorPickerState>,
    input: &Entity<InputState>,
    none: Option<Stateful<Div>>,
    cx: &mut App,
) -> impl IntoElement {
    let dot = ColorDot {
        id: id.into(),
        color: color.map(theme::hsla_of),
        selected: false,
    };

    let open_picker = picker.clone();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .child(fixed_label(label))
        .child(
            Popover::new(id)
                .anchor(Anchor::BottomLeft)
                .open(picker.read(cx).is_open())
                .on_open_change(move |open, _window, cx| {
                    let open = *open;
                    open_picker.update(cx, |state, cx| state.set_open(open, cx));
                })
                .trigger(dot)
                .child(color_picker_panel(id, picker, color, none, cx)),
        )
        .child(
            div()
                .w(px(100.0))
                .child(gpui_kit::component::input::Input::new(input).h(px(28.0))),
        )
}

/// The shared dot both colour settings use: a round swatch that opens the
/// picker popup, or the "no colour" symbol when the slot has no colour.
///
/// [`Popover`] wants a [`Selectable`] trigger, and the kit's own swatch is not
/// one — it carries radio semantics for a palette instead. This wrapper draws
/// nothing but the colour, and reports the popup's open state as its selected
/// state, which is what the popover draws the ring from.
struct ColorDot {
    id: ElementId,
    color: Option<Hsla>,
    selected: bool,
}

impl Selectable for ColorDot {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl IntoElement for ColorDot {
    type Element = AnyElement;

    fn into_element(self) -> AnyElement {
        let Some(color) = self.color else {
            return div()
                .id(self.id)
                .cursor_pointer()
                .child(none_color_icon(26.0, self.selected))
                .into_any_element();
        };
        let ring = if self.selected {
            theme::primary()
        } else {
            swatch_edge(color).into()
        };
        div()
            .id(self.id)
            .w(px(26.0))
            .h(px(26.0))
            .rounded_full()
            .bg(color)
            .border(px(if self.selected { 2.0 } else { 1.0 }))
            .border_color(ring)
            .cursor_pointer()
            .hover(|style| style.border_color(theme::primary()))
            .into_any_element()
    }
}

/// The symbol every palette uses for "none": a rounded square with a diagonal
/// through it, drawn rather than shipped as an SVG so it takes its colours from
/// the theme and matches the swatches it sits among.
pub fn none_color_icon(size: f32, selected: bool) -> impl IntoElement {
    let edge = if selected { 2.0 } else { 1.0 };
    // An empty tile needs a visible outline; the near-white hairline the other
    // controls use would leave it all but invisible on the popup.
    let border = if selected {
        theme::primary()
    } else {
        theme::border()
    };
    let slash = theme::text_color();
    let inset = (size * 0.28).round();
    canvas(
        move |bounds, _window, _cx| bounds,
        move |bounds, _bounds, window, _cx| {
            window.paint_quad(PaintQuad {
                bounds,
                corner_radii: Corners::all(px(theme::RADIUS_SM)),
                background: theme::field_bg().into(),
                border_widths: Edges::all(px(edge)),
                border_color: border.into(),
                border_style: BorderStyle::default(),
            });
            let mut diagonal = PathBuilder::stroke(px(edge + 0.8));
            diagonal.move_to(bounds.origin + point(px(inset), bounds.size.height - px(inset)));
            diagonal.line_to(bounds.origin + point(bounds.size.width - px(inset), px(inset)));
            if let Ok(path) = diagonal.build() {
                window.paint_path(path, slash);
            }
        },
    )
    .w(px(size))
    .h(px(size))
}

/// The clickable "no colour" candidate for a picker's palette: the same symbol
/// on a swatch-sized tile, which is what the page wires its own meaning to.
pub fn none_color_candidate(
    id: impl Into<ElementId>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .child(none_color_icon(24.0, selected))
        .on_click(on_click)
}

/// The colours offered under the sliders: the neutrals and the two Altium
/// defaults first, then two rows of the hues these palettes use most.
const COLOR_PRESETS: [[u8; 3]; 24] = [
    [0, 0, 0],
    [64, 64, 64],
    [128, 128, 128],
    [192, 192, 192],
    [255, 255, 255],
    [11, 56, 104],
    [255, 255, 224],
    [0, 92, 175],
    [196, 30, 58],
    [214, 93, 14],
    [240, 180, 0],
    [76, 154, 42],
    [0, 132, 132],
    [24, 76, 168],
    [112, 48, 160],
    [190, 24, 93],
    [255, 138, 128],
    [255, 196, 128],
    [255, 236, 128],
    [168, 220, 128],
    [128, 222, 214],
    [128, 178, 255],
    [196, 160, 255],
    [255, 160, 200],
];

/// The hairline around a colour swatch: the colour itself, darkened, rather
/// than the pale grey the other controls use.
///
/// A near-white hairline anti-aliases into the fill and leaves a light fringe
/// around it, which on a saturated tile reads as a ragged, pixelated edge. A
/// darker shade of the same colour blends into the fill instead, so the tile
/// keeps one crisp contour — and a light tile still gets an edge to sit behind.
fn swatch_edge(color: Hsla) -> Hsla {
    color.darken(0.22)
}

/// The body of the colour picker: one slider per H/S/L component and the preset
/// palette under them.
///
/// Every control writes into `picker`, which emits the change the page folds
/// back into its own state, so the swatch, the hex box and the export all see
/// the same colour.
fn color_picker_panel(
    id: &'static str,
    picker: &Entity<ColorPickerState>,
    color: Option<[u8; 3]>,
    none: Option<Stateful<Div>>,
    cx: &mut App,
) -> AnyElement {
    let sliders = picker.read(cx).sliders().clone();

    let slider_row = |title: &'static str, state: &Entity<SliderState>| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .w_full()
            .child(
                div()
                    .w(px(14.0))
                    .text_size(px(SMALL))
                    .text_color(theme::muted())
                    .child(title),
            )
            .child(div().flex_1().child(Slider::new(state)))
    };

    let mut palette = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(6.0))
        .w(px(238.0));
    // The "no colour" candidate leads the palette, where a palette keeps it.
    // While it is the choice, no colour preset is marked as selected.
    if let Some(none) = none {
        palette = palette.child(none);
    }
    for (index, preset) in COLOR_PRESETS.iter().enumerate() {
        let swatch = theme::hsla_of(*preset);
        let selected = color == Some(*preset);
        let picker = picker.clone();
        let mut tile = ColorSwatch::new(SharedString::from(format!("{id}-preset-{index}")), swatch)
            .bg(swatch)
            .w(px(24.0))
            .h(px(24.0))
            .rounded(px(theme::RADIUS_SM))
            .border(px(1.0))
            .border_color(swatch_edge(swatch))
            .selected(selected)
            .cursor_pointer()
            .on_click(move |color, _event, window, cx| {
                picker.update(cx, |state, cx| state.select_color(color, window, cx));
            });
        if selected {
            // `selected` above is the state assistive technology reads; the ring
            // the eye needs is drawn here, as a zero-blur shadow spread outside
            // the tile, which leaves the tile's own colours untouched.
            tile = tile.shadow(vec![BoxShadow {
                color: theme::primary().into(),
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: px(2.0),
                inset: false,
            }]);
        }
        palette = palette.child(tile);
    }

    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(slider_row("H", sliders.hue()))
        .child(slider_row("S", sliders.saturation()))
        .child(slider_row("L", sliders.lightness()))
        .child(palette)
        .into_any_element()
}
