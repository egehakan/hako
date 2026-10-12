//! The dialog in which a tab group's color and icon are chosen.

use pathfinder_color::ColorU;
use pathfinder_geometry::vector::vec2f;
use warp_core::ui::theme::Fill;
use warpui::elements::{
    Align, Border, ChildAnchor, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    Empty, Flex, Hoverable, MainAxisSize, MouseStateHandle, OffsetPositioning, ParentAnchor,
    ParentElement, ParentOffsetBounds, Radius, Stack, Text,
};
use warpui::fonts::Weight;
use warpui::keymap::FixedBinding;
use warpui::platform::Cursor;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
};

use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, SingleLineEditorOptions, TextOptions,
};
use crate::ui_components::dialog::{Dialog, dialog_styles};
use crate::workspace::tab_folders::{
    MATERIAL_GROUP_ICONS, TabFolders, TabGroupIcon, color_swatch_rows, format_hex_color,
    parse_hex_color,
};
use crate::workspace::tab_group::{TabGroup, TabGroupId};

const SWATCH_SIZE: f32 = 20.;
const ICON_CELL_SIZE: f32 = 26.;
const ICON_SIZE: f32 = 18.;
const ICONS_PER_ROW: usize = 16;
const GRID_GAP: f32 = 4.;
const INPUT_WIDTH: f32 = 120.;

pub(super) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings(vec![FixedBinding::new(
        "escape",
        TabGroupCustomizeAction::Close,
        id!("TabGroupCustomizeDialog"),
    )]);
}

pub enum TabGroupCustomizeEvent {
    SetColor {
        group_id: TabGroupId,
        color: Option<ColorU>,
    },
    SetIcon {
        group_id: TabGroupId,
        icon: Option<TabGroupIcon>,
    },
    Close,
}

#[derive(Debug, Clone)]
pub enum TabGroupCustomizeAction {
    PickColor(Option<ColorU>),
    PickIcon(Option<TabGroupIcon>),
    Close,
}

/// Edits the group it was opened for. Every pick applies at once, so the tab list behind the
/// dialog shows the result; there is nothing to confirm or to cancel.
pub struct TabGroupCustomizeDialog {
    group_id: Option<TabGroupId>,
    group_name: String,
    color: Option<ColorU>,
    icon: Option<TabGroupIcon>,
    /// Set while the hex field holds something that is not a color.
    hex_is_invalid: bool,
    hex_editor: ViewHandle<EditorView>,
    icon_text_editor: ViewHandle<EditorView>,
    swatch_rows: Vec<Vec<ColorU>>,
    swatch_mouse_states: Vec<MouseStateHandle>,
    recent_mouse_states: Vec<MouseStateHandle>,
    icon_mouse_states: Vec<MouseStateHandle>,
    no_color_mouse_state: MouseStateHandle,
    default_icon_mouse_state: MouseStateHandle,
    done_mouse_state: MouseStateHandle,
}

impl TabGroupCustomizeDialog {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let hex_editor = Self::single_line_editor("#RRGGBB", ctx);
        ctx.subscribe_to_view(&hex_editor, |me, _, event, ctx| {
            me.handle_hex_editor_event(event, ctx);
        });
        let icon_text_editor = Self::single_line_editor("Emoji or text", ctx);
        ctx.subscribe_to_view(&icon_text_editor, |me, _, event, ctx| {
            me.handle_icon_text_editor_event(event, ctx);
        });

        let swatch_rows = color_swatch_rows();
        let swatch_count = swatch_rows.iter().map(Vec::len).sum();
        Self {
            group_id: None,
            group_name: String::new(),
            color: None,
            icon: None,
            hex_is_invalid: false,
            hex_editor,
            icon_text_editor,
            swatch_rows,
            swatch_mouse_states: mouse_states(swatch_count),
            recent_mouse_states: Vec::new(),
            icon_mouse_states: mouse_states(MATERIAL_GROUP_ICONS.len()),
            no_color_mouse_state: Default::default(),
            default_icon_mouse_state: Default::default(),
            done_mouse_state: Default::default(),
        }
    }

    fn single_line_editor(
        placeholder: &'static str,
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<EditorView> {
        ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = SingleLineEditorOptions {
                text: TextOptions::ui_text(Some(13.), appearance),
                ..Default::default()
            };
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text(placeholder, ctx);
            editor
        })
    }

    /// Points the dialog at `group` and focuses the hex field.
    pub fn open_for_group(&mut self, group: &TabGroup, ctx: &mut ViewContext<Self>) {
        self.group_id = Some(group.id);
        self.group_name = group.display_name();
        self.color = group.custom_color;
        self.icon = group.icon.clone();
        self.hex_is_invalid = false;
        self.recent_mouse_states = mouse_states(TabFolders::as_ref(ctx).recent_colors().len());

        let hex = self.color.map(format_hex_color).unwrap_or_default();
        self.hex_editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(&hex, ctx);
        });
        let icon_text = match &self.icon {
            Some(TabGroupIcon::Text(text)) => text.clone(),
            Some(TabGroupIcon::Material(_)) | None => String::new(),
        };
        self.icon_text_editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(&icon_text, ctx);
        });
        ctx.focus(&self.hex_editor);
        ctx.notify();
    }

    fn handle_hex_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Edited(_) => {
                let text = self.hex_editor.as_ref(ctx).buffer_text(ctx);
                // A color applies as soon as it is complete, so the tab list previews it while
                // typing. The short `#RGB` form waits for Enter: it is a prefix of the long one.
                let color = parse_hex_color(&text)
                    .filter(|_| text.trim().trim_start_matches('#').len() == 6);
                self.hex_is_invalid = false;
                if let Some(color) = color
                    && self.color != Some(color)
                {
                    self.apply_color(Some(color), ctx);
                }
                ctx.notify();
            }
            EditorEvent::Enter => {
                let text = self.hex_editor.as_ref(ctx).buffer_text(ctx);
                if text.trim().is_empty() {
                    ctx.emit(TabGroupCustomizeEvent::Close);
                    return;
                }
                match parse_hex_color(&text) {
                    Some(color) => {
                        self.hex_is_invalid = false;
                        self.apply_color(Some(color), ctx);
                        self.show_color_in_hex_editor(ctx);
                    }
                    None => self.hex_is_invalid = true,
                }
                ctx.notify();
            }
            EditorEvent::Escape => ctx.emit(TabGroupCustomizeEvent::Close),
            _ => {}
        }
    }

    fn handle_icon_text_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Edited(_) => {
                let text = self.icon_text_editor.as_ref(ctx).buffer_text(ctx);
                // Clearing the field must not wipe a picked icon, so only text is applied.
                if let Some(icon) = TabGroupIcon::text(&text)
                    && self.icon.as_ref() != Some(&icon)
                {
                    self.apply_icon(Some(icon), ctx);
                }
            }
            EditorEvent::Escape => ctx.emit(TabGroupCustomizeEvent::Close),
            _ => {}
        }
    }

    fn apply_color(&mut self, color: Option<ColorU>, ctx: &mut ViewContext<Self>) {
        let Some(group_id) = self.group_id else {
            return;
        };
        self.color = color;
        ctx.emit(TabGroupCustomizeEvent::SetColor { group_id, color });
        ctx.notify();
    }

    fn apply_icon(&mut self, icon: Option<TabGroupIcon>, ctx: &mut ViewContext<Self>) {
        let Some(group_id) = self.group_id else {
            return;
        };
        self.icon = icon.clone();
        ctx.emit(TabGroupCustomizeEvent::SetIcon { group_id, icon });
        ctx.notify();
    }

    fn show_color_in_hex_editor(&mut self, ctx: &mut ViewContext<Self>) {
        let hex = self.color.map(format_hex_color).unwrap_or_default();
        self.hex_editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(&hex, ctx);
        });
    }

    fn render_section_label(text: &'static str, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        Container::new(
            Text::new_inline(text, appearance.ui_font_family(), 12.)
                .with_color(theme.sub_text_color(theme.surface_2()).into())
                .finish(),
        )
        .with_margin_bottom(6.)
        .finish()
    }

    fn render_swatch(
        &self,
        color: ColorU,
        mouse_state: MouseStateHandle,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let is_selected = self.color == Some(color);
        let ring: ColorU = if is_selected {
            appearance.theme().accent().into()
        } else {
            ColorU::transparent_black()
        };
        Hoverable::new(mouse_state, move |_| {
            Container::new(
                Container::new(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(SWATCH_SIZE - 6.)
                        .with_height(SWATCH_SIZE - 6.)
                        .finish(),
                )
                .with_background_color(color)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.)))
                .finish(),
            )
            .with_padding(warpui::elements::Padding::uniform(1.))
            .with_border(Border::all(2.).with_border_color(ring))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(TabGroupCustomizeAction::PickColor(Some(color)));
        })
        .finish()
    }

    fn render_color_section(&self, appearance: &Appearance, app: &AppContext) -> Box<dyn Element> {
        let theme = appearance.theme();
        let mut column = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start);
        column.add_child(Self::render_section_label("Color", appearance));

        let preview: ColorU = self.color.unwrap_or_else(ColorU::transparent_black);
        let hex_input = ConstrainedBox::new(
            appearance
                .ui_builder()
                .text_input(self.hex_editor.clone())
                .build()
                .finish(),
        )
        .with_width(INPUT_WIDTH)
        .finish();
        let no_color_button = appearance
            .ui_builder()
            .button(ButtonVariant::Basic, self.no_color_mouse_state.clone())
            .with_centered_text_label("No color".into())
            .with_style(small_button_style())
            .build()
            .with_cursor(Cursor::PointingHand)
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(TabGroupCustomizeAction::PickColor(None));
            })
            .finish();
        let mut input_row = Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.)
            .with_child(hex_input)
            .with_child(
                Container::new(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(22.)
                        .with_height(22.)
                        .finish(),
                )
                .with_background_color(preview)
                .with_border(Border::all(1.).with_border_fill(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
                .finish(),
            )
            .with_child(no_color_button);
        if self.hex_is_invalid {
            input_row.add_child(
                Text::new_inline(
                    "Not a color. Use #RRGGBB or #RGB.",
                    appearance.ui_font_family(),
                    12.,
                )
                .with_color(theme.ui_error_color())
                .finish(),
            );
        }
        column.add_child(
            Container::new(input_row.finish())
                .with_margin_bottom(10.)
                .finish(),
        );

        let mut mouse_states = self.swatch_mouse_states.iter().cloned();
        for row in &self.swatch_rows {
            let mut swatches = Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(GRID_GAP);
            for color in row {
                let Some(mouse_state) = mouse_states.next() else {
                    break;
                };
                swatches.add_child(self.render_swatch(*color, mouse_state, appearance));
            }
            column.add_child(
                Container::new(swatches.finish())
                    .with_margin_bottom(GRID_GAP)
                    .finish(),
            );
        }

        let recent_colors = TabFolders::as_ref(app).recent_colors();
        if !recent_colors.is_empty() {
            let mut recents = Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(GRID_GAP)
                .with_child(
                    Container::new(
                        Text::new_inline("Recent", appearance.ui_font_family(), 12.)
                            .with_color(theme.sub_text_color(theme.surface_2()).into())
                            .finish(),
                    )
                    .with_margin_right(6.)
                    .finish(),
                );
            for (color, mouse_state) in recent_colors.iter().zip(&self.recent_mouse_states) {
                recents.add_child(self.render_swatch(*color, mouse_state.clone(), appearance));
            }
            column.add_child(
                Container::new(recents.finish())
                    .with_margin_top(4.)
                    .finish(),
            );
        }
        column.finish()
    }

    fn render_icon_section(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let mut column = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start);
        column.add_child(Self::render_section_label("Icon", appearance));

        let text_input = ConstrainedBox::new(
            appearance
                .ui_builder()
                .text_input(self.icon_text_editor.clone())
                .build()
                .finish(),
        )
        .with_width(INPUT_WIDTH)
        .finish();
        let default_icon_button = appearance
            .ui_builder()
            .button(ButtonVariant::Basic, self.default_icon_mouse_state.clone())
            .with_centered_text_label("Default icon".into())
            .with_style(small_button_style())
            .build()
            .with_cursor(Cursor::PointingHand)
            .on_click(|ctx, _, _| {
                ctx.dispatch_typed_action(TabGroupCustomizeAction::PickIcon(None));
            })
            .finish();
        column.add_child(
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.)
                    .with_child(text_input)
                    .with_child(default_icon_button)
                    .finish(),
            )
            .with_margin_bottom(10.)
            .finish(),
        );

        let accent: ColorU = theme.accent().into();
        for (row_index, row) in MATERIAL_GROUP_ICONS.chunks(ICONS_PER_ROW).enumerate() {
            let mut icons = Flex::row().with_main_axis_size(MainAxisSize::Min);
            for (column_index, (key, _)) in row.iter().enumerate() {
                let Some(icon) = TabGroupIcon::material(key) else {
                    continue;
                };
                let Some(mouse_state) = self
                    .icon_mouse_states
                    .get(row_index * ICONS_PER_ROW + column_index)
                    .cloned()
                else {
                    continue;
                };
                let is_selected = self.icon.as_ref() == Some(&icon);
                let hover_fill = theme.surface_3();
                let picked = icon.clone();
                icons.add_child(
                    Hoverable::new(mouse_state, move |state| {
                        let image = icon
                            .material_image()
                            .unwrap_or_else(|| Empty::new().finish());
                        let mut cell = Container::new(
                            ConstrainedBox::new(image)
                                .with_width(ICON_SIZE)
                                .with_height(ICON_SIZE)
                                .finish(),
                        )
                        .with_padding(warpui::elements::Padding::uniform(
                            (ICON_CELL_SIZE - ICON_SIZE) / 2. - 1.,
                        ))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
                        .with_border(Border::all(1.).with_border_color(if is_selected {
                            accent
                        } else {
                            ColorU::transparent_black()
                        }));
                        if state.is_hovered() || is_selected {
                            cell = cell.with_background(hover_fill);
                        }
                        cell.finish()
                    })
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(TabGroupCustomizeAction::PickIcon(Some(
                            picked.clone(),
                        )));
                    })
                    .finish(),
                );
            }
            column.add_child(icons.finish());
        }
        column.finish()
    }
}

fn mouse_states(count: usize) -> Vec<MouseStateHandle> {
    (0..count).map(|_| MouseStateHandle::default()).collect()
}

fn small_button_style() -> UiComponentStyles {
    UiComponentStyles {
        font_size: Some(12.),
        height: Some(26.),
        padding: Some(Coords {
            top: 0.,
            bottom: 0.,
            left: 10.,
            right: 10.,
        }),
        ..Default::default()
    }
}

impl Entity for TabGroupCustomizeDialog {
    type Event = TabGroupCustomizeEvent;
}

impl View for TabGroupCustomizeDialog {
    fn ui_name() -> &'static str {
        "TabGroupCustomizeDialog"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);

        let done_button = appearance
            .ui_builder()
            .button(ButtonVariant::Accent, self.done_mouse_state.clone())
            .with_centered_text_label("Done".into())
            .with_style(UiComponentStyles {
                font_size: Some(14.),
                font_weight: Some(Weight::Bold),
                width: Some(120.),
                height: Some(36.),
                ..Default::default()
            })
            .build()
            .with_cursor(Cursor::PointingHand)
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(TabGroupCustomizeAction::Close))
            .finish();

        let body = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(
                Container::new(self.render_color_section(appearance, app))
                    .with_margin_bottom(16.)
                    .finish(),
            )
            .with_child(self.render_icon_section(appearance))
            .finish();

        let dialog = Container::new(
            Dialog::new(
                format!("Customize \"{}\"", self.group_name),
                None,
                UiComponentStyles {
                    width: Some(ICONS_PER_ROW as f32 * ICON_CELL_SIZE + 48.),
                    padding: Some(Coords::uniform(24.)),
                    ..dialog_styles(appearance)
                },
            )
            .with_child(body)
            .with_bottom_row_child(done_button)
            .build()
            .finish(),
        )
        .with_margin_top(35.)
        .finish();

        let mut stack = Stack::new();
        stack.add_positioned_child(
            dialog,
            OffsetPositioning::offset_from_parent(
                vec2f(0., 0.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            ),
        );

        // No blur behind this dialog: the point is to watch the tab list change.
        Container::new(Align::new(stack.finish()).finish())
            .with_background_color(Fill::Solid(ColorU::new(0, 0, 0, 90)).into())
            .with_corner_radius(app.windows().window_corner_radius())
            .finish()
    }
}

impl TypedActionView for TabGroupCustomizeDialog {
    type Action = TabGroupCustomizeAction;

    fn handle_action(&mut self, action: &TabGroupCustomizeAction, ctx: &mut ViewContext<Self>) {
        match action {
            TabGroupCustomizeAction::PickColor(color) => {
                self.hex_is_invalid = false;
                self.apply_color(*color, ctx);
                self.show_color_in_hex_editor(ctx);
            }
            TabGroupCustomizeAction::PickIcon(icon) => {
                // A picked icon replaces any text icon, so the text field is emptied to match.
                self.icon_text_editor.update(ctx, |editor, ctx| {
                    editor.set_buffer_text("", ctx);
                });
                self.apply_icon(icon.clone(), ctx);
            }
            TabGroupCustomizeAction::Close => ctx.emit(TabGroupCustomizeEvent::Close),
        }
    }
}
