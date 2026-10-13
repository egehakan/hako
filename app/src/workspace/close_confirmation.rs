//! The confirmation asked before a pane, tab, group or window is closed.

use pathfinder_geometry::vector::vec2f;
use warp_core::ui::theme::Fill;
use warpui::elements::{
    Align, ChildAnchor, Container, MouseStateHandle, OffsetPositioning, ParentAnchor,
    ParentOffsetBounds, Stack,
};
use warpui::fonts::Weight;
use warpui::keymap::FixedBinding;
use warpui::platform::Cursor;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext};

use crate::appearance::Appearance;
use crate::channel::{Channel, ChannelState};
use crate::terminal::general_settings::GeneralSettings;
use crate::ui_components::dialog::{Dialog, dialog_styles};

pub(super) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings(vec![
        FixedBinding::new(
            "escape",
            CloseConfirmationAction::Cancel,
            id!("CloseConfirmationDialog"),
        ),
        FixedBinding::new(
            "enter",
            CloseConfirmationAction::ActivateFocusedButton,
            id!("CloseConfirmationDialog"),
        ),
        FixedBinding::new(
            "tab",
            CloseConfirmationAction::FocusOtherButton,
            id!("CloseConfirmationDialog"),
        ),
        FixedBinding::new(
            "shift-tab",
            CloseConfirmationAction::FocusOtherButton,
            id!("CloseConfirmationDialog"),
        ),
        FixedBinding::new(
            "left",
            CloseConfirmationAction::FocusOtherButton,
            id!("CloseConfirmationDialog"),
        ),
        FixedBinding::new(
            "right",
            CloseConfirmationAction::FocusOtherButton,
            id!("CloseConfirmationDialog"),
        ),
    ]);
}

/// Whether every close asks for confirmation first. Integration tests cannot press buttons, so
/// they never get the dialog.
pub fn confirms_every_close(app: &AppContext) -> bool {
    *GeneralSettings::as_ref(app).confirm_before_closing
        && ChannelState::channel() != Channel::Integration
}

/// Whether closing a single pane asks first while every close is to be confirmed. A pane of
/// files does not: all that closing one can lose is unsaved changes, which are asked about
/// separately and whatever this setting says.
pub fn confirms_pane_close(always_confirm: bool, holds_files: bool) -> bool {
    always_confirm && !holds_files
}

/// What is about to be closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseSubject {
    Pane {
        title: String,
    },
    Tab {
        title: String,
        /// The tab is the last one in its window, which closes with it.
        closes_window: bool,
    },
    Tabs {
        count: usize,
    },
    /// The tabs of a group, and with `removes_group` the group itself.
    Group {
        name: String,
        tab_count: usize,
        removes_group: bool,
    },
    Window {
        tab_count: usize,
    },
}

/// What a pane holds, as far as naming it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneLabel {
    /// An editor or file viewer, with the name of the file in its active tab.
    File {
        file_name: Option<String>,
    },
    Terminal {
        /// Where the session is, for a terminal that has no title yet.
        directory: Option<String>,
    },
    Other,
}

/// The name a pane goes by in the confirmation, given its title, which is empty for a pane that
/// has not got round to setting one: a terminal still starting its shell, an editor still
/// loading its file. The name is never empty.
pub fn pane_label(title: &str, pane: PaneLabel) -> String {
    let non_empty = |name: &str| Some(name.trim().to_string()).filter(|name| !name.is_empty());
    let title = non_empty(title);
    match pane {
        PaneLabel::File { file_name } => file_name
            .as_deref()
            .and_then(non_empty)
            .or(title)
            .unwrap_or_else(|| "Untitled".to_string()),
        PaneLabel::Terminal { directory } => title
            .or_else(|| directory.as_deref().and_then(non_empty))
            .unwrap_or_else(|| "Terminal".to_string()),
        PaneLabel::Other => title.unwrap_or_else(|| "Untitled".to_string()),
    }
}

/// What would be lost by closing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloseStakes {
    pub running_commands: usize,
    pub shared_sessions: usize,
    pub unsaved_changes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseConfirmationText {
    pub title: String,
    pub body: String,
    pub confirm_label: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseDecision {
    /// Ask with this text, whatever is or is not running.
    Ask(CloseConfirmationText),
    /// Leave it to the warnings that only appear when something would be lost.
    WarnOnlyIfSomethingIsLost,
}

/// Decides how a close request is handled, given the "Always confirm before closing" setting.
pub fn close_decision(
    subject: &CloseSubject,
    stakes: CloseStakes,
    always_confirm: bool,
) -> CloseDecision {
    if always_confirm {
        CloseDecision::Ask(confirmation_text(subject, stakes))
    } else {
        CloseDecision::WarnOnlyIfSomethingIsLost
    }
}

fn tabs(count: usize) -> String {
    if count == 1 {
        "1 tab".to_string()
    } else {
        format!("{count} tabs")
    }
}

/// Names `kind` by `name` in quotes, or without a name when there is none to quote.
fn named(kind: &str, name: &str) -> String {
    match name.trim() {
        "" => format!("This {kind}"),
        name => format!("The {kind} \"{name}\""),
    }
}

fn confirmation_text(subject: &CloseSubject, stakes: CloseStakes) -> CloseConfirmationText {
    let (title, mut body, confirm_label) = match subject {
        CloseSubject::Pane { title } => (
            "Close this pane?".to_string(),
            format!("{} will be closed.", named("pane", title)),
            "Close pane",
        ),
        CloseSubject::Tab {
            title,
            closes_window,
        } => {
            let mut body = format!("{} will be closed.", named("tab", title));
            if *closes_window {
                body.push_str(" It is the last tab, so the window closes with it.");
            }
            ("Close this tab?".to_string(), body, "Close tab")
        }
        CloseSubject::Tabs { count } => (
            format!("Close {}?", tabs(*count)),
            format!("{} will be closed.", tabs(*count)),
            "Close tabs",
        ),
        CloseSubject::Group {
            name,
            tab_count,
            removes_group: true,
        } => {
            let body = if *tab_count == 0 {
                "The group will be removed from the tab list.".to_string()
            } else {
                format!(
                    "The group will be removed from the tab list and its {} closed.",
                    tabs(*tab_count)
                )
            };
            let question = match name.trim() {
                "" => "Remove this group?".to_string(),
                name => format!("Remove the group \"{name}\"?"),
            };
            (
                question,
                format!("{body} Nothing on disk is deleted."),
                "Remove group",
            )
        }
        CloseSubject::Group {
            name,
            tab_count,
            removes_group: false,
        } => (
            match name.trim() {
                "" => "Close all tabs in this group?".to_string(),
                name => format!("Close all tabs in \"{name}\"?"),
            },
            format!(
                "{} will be closed. The group stays in the tab list.",
                tabs(*tab_count)
            ),
            "Close tabs",
        ),
        CloseSubject::Window { tab_count } => (
            "Close this window?".to_string(),
            format!("The window and its {} will be closed.", tabs(*tab_count)),
            "Close window",
        ),
    };

    match stakes.running_commands {
        0 => {}
        1 => body.push_str(" A command is still running."),
        count => body.push_str(&format!(" {count} commands are still running.")),
    }
    match stakes.shared_sessions {
        0 => {}
        1 => body.push_str(" A shared session will end for everyone."),
        count => body.push_str(&format!(" {count} shared sessions will end for everyone.")),
    }
    if stakes.unsaved_changes {
        body.push_str(" Unsaved changes will be lost.");
    }

    CloseConfirmationText {
        title,
        body,
        confirm_label,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DialogButton {
    Cancel,
    Confirm,
}

/// Asks whether to go through with a close. Cancel has the focus when the dialog opens, so that
/// Enter keeps everything open unless the focus is moved first.
pub struct CloseConfirmationDialog {
    text: Option<CloseConfirmationText>,
    focused_button: DialogButton,
    cancel_mouse_state: MouseStateHandle,
    confirm_mouse_state: MouseStateHandle,
}

impl CloseConfirmationDialog {
    pub fn new() -> Self {
        Self {
            text: None,
            focused_button: DialogButton::Cancel,
            cancel_mouse_state: Default::default(),
            confirm_mouse_state: Default::default(),
        }
    }

    pub fn show(&mut self, text: CloseConfirmationText, ctx: &mut ViewContext<Self>) {
        self.text = Some(text);
        self.focused_button = DialogButton::Cancel;
        ctx.notify();
    }

    #[cfg(test)]
    pub fn text(&self) -> Option<&CloseConfirmationText> {
        self.text.as_ref()
    }
}

impl Default for CloseConfirmationDialog {
    fn default() -> Self {
        Self::new()
    }
}

pub enum CloseConfirmationEvent {
    Confirmed,
    Cancelled,
}

#[derive(Debug)]
pub enum CloseConfirmationAction {
    Cancel,
    Confirm,
    ActivateFocusedButton,
    FocusOtherButton,
}

impl Entity for CloseConfirmationDialog {
    type Event = CloseConfirmationEvent;
}

impl View for CloseConfirmationDialog {
    fn ui_name() -> &'static str {
        "CloseConfirmationDialog"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let button_style = UiComponentStyles {
            font_size: Some(14.),
            font_weight: Some(Weight::Bold),
            width: Some(202.),
            height: Some(40.),
            ..Default::default()
        };
        // The focused button is the highlighted one, which is the one Enter activates.
        let variant = |button| {
            if self.focused_button == button {
                ButtonVariant::Accent
            } else {
                ButtonVariant::Basic
            }
        };
        let (title, body, confirm_label) = match &self.text {
            Some(text) => (text.title.clone(), text.body.clone(), text.confirm_label),
            None => (String::new(), String::new(), "Close"),
        };

        let cancel_button = appearance
            .ui_builder()
            .button(
                variant(DialogButton::Cancel),
                self.cancel_mouse_state.clone(),
            )
            .with_centered_text_label("Cancel".into())
            .with_style(button_style)
            .build()
            .with_cursor(Cursor::PointingHand)
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(CloseConfirmationAction::Cancel))
            .finish();
        let confirm_button = appearance
            .ui_builder()
            .button(
                variant(DialogButton::Confirm),
                self.confirm_mouse_state.clone(),
            )
            .with_centered_text_label(confirm_label.into())
            .with_style(button_style)
            .build()
            .with_cursor(Cursor::PointingHand)
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(CloseConfirmationAction::Confirm))
            .finish();

        let dialog = Container::new(
            Dialog::new(
                title,
                Some(body),
                UiComponentStyles {
                    width: Some(460.),
                    padding: Some(Coords::uniform(24.)),
                    ..dialog_styles(appearance)
                },
            )
            .with_bottom_row_child(cancel_button)
            .with_bottom_row_child(confirm_button)
            .build()
            .finish(),
        )
        .with_margin_top(35.)
        .finish();

        // The stack gives the dialog window bounds, which keep it clear of the traffic lights.
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

        Container::new(Align::new(stack.finish()).finish())
            .with_background_color(Fill::blur().into())
            .with_corner_radius(app.windows().window_corner_radius())
            .finish()
    }
}

impl TypedActionView for CloseConfirmationDialog {
    type Action = CloseConfirmationAction;

    fn handle_action(&mut self, action: &CloseConfirmationAction, ctx: &mut ViewContext<Self>) {
        match action {
            CloseConfirmationAction::Cancel => ctx.emit(CloseConfirmationEvent::Cancelled),
            CloseConfirmationAction::Confirm => ctx.emit(CloseConfirmationEvent::Confirmed),
            CloseConfirmationAction::ActivateFocusedButton => match self.focused_button {
                DialogButton::Cancel => ctx.emit(CloseConfirmationEvent::Cancelled),
                DialogButton::Confirm => ctx.emit(CloseConfirmationEvent::Confirmed),
            },
            CloseConfirmationAction::FocusOtherButton => {
                self.focused_button = match self.focused_button {
                    DialogButton::Cancel => DialogButton::Confirm,
                    DialogButton::Confirm => DialogButton::Cancel,
                };
                ctx.notify();
            }
        }
    }
}

#[cfg(test)]
#[path = "close_confirmation_tests.rs"]
mod tests;
