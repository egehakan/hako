use std::cell::RefCell;
use std::collections::HashMap;

use hako_remote::agent_endpoint;
use settings::{Setting, ToggleableSetting};
use warp_errors::report_if_error;
use warpui::elements::{
    Container, CrossAxisAlignment, Flex, MainAxisAlignment, MouseStateHandle, ParentElement, Text,
};
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::switch::SwitchStateHandle;
use warpui::{
    AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
};

use crate::appearance::Appearance;
use crate::editor::{self, EditorView, SingleLineEditorOptions, TextOptions};
use crate::hako_remote::{HakoRemote, RemoteAccessStatus};
use crate::settings::{
    DEFAULT_REMOTE_ACCESS_SERVER_ADDRESS, RemoteAccessEnabled, RemoteAccessSettings,
};
use crate::settings_view::features_page::render_group;
use crate::settings_view::settings_page::{LocalOnlyIconState, ToggleState, render_body_item};

const SERVER_ADDRESS_EDITOR_WIDTH: f32 = 280.;

#[cfg(target_os = "macos")]
const KEEP_AWAKE_NOTE: &str =
    "\n\nKeeps your Mac awake while remote access is on. Closing the lid still puts it to sleep.";
#[cfg(not(target_os = "macos"))]
const KEEP_AWAKE_NOTE: &str = "";

#[derive(Debug, Clone, Copy)]
pub enum Action {
    ToggleEnabled,
    UpdateServerAddress,
}

/// The remote access settings: the switch, the server address and how the connection is doing.
pub struct RemoteAccessView {
    switch_state: SwitchStateHandle,
    server_address_editor: ViewHandle<EditorView>,
    is_server_address_valid: bool,
    local_only_icon_states: RefCell<HashMap<String, MouseStateHandle>>,
}

impl RemoteAccessView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let server_address_editor = ctx.add_typed_action_view(|ctx| {
            EditorView::single_line(
                SingleLineEditorOptions {
                    text: TextOptions::ui_font_size(Appearance::as_ref(ctx)),
                    ..Default::default()
                },
                ctx,
            )
        });
        let server_address = RemoteAccessSettings::as_ref(ctx)
            .server_address
            .value()
            .clone();
        server_address_editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(&server_address, ctx);
        });

        ctx.subscribe_to_model(&RemoteAccessSettings::handle(ctx), |me, _, _, ctx| {
            let server_address = RemoteAccessSettings::as_ref(ctx)
                .server_address
                .value()
                .clone();
            me.server_address_editor.update(ctx, |editor, ctx| {
                editor.set_buffer_text(&server_address, ctx);
            });
            ctx.notify();
        });
        ctx.subscribe_to_view(&server_address_editor, |me, _, event, ctx| {
            me.handle_editor_event(event, ctx);
        });
        if ctx.has_singleton_model::<HakoRemote>() {
            ctx.subscribe_to_model(&HakoRemote::handle(ctx), |_, _, _, ctx| ctx.notify());
        }

        Self {
            switch_state: Default::default(),
            server_address_editor,
            is_server_address_valid: true,
            local_only_icon_states: Default::default(),
        }
    }

    /// Saves the address when the user presses Enter or leaves the field.
    fn handle_editor_event(&mut self, event: &editor::Event, ctx: &mut ViewContext<Self>) {
        use editor::Event;
        match event {
            Event::Edited(_) => {
                let text = self.server_address_editor.as_ref(ctx).buffer_text(ctx);
                let is_valid = Self::parse_server_address(&text).is_some();
                if is_valid != self.is_server_address_valid {
                    self.is_server_address_valid = is_valid;
                    ctx.notify();
                }
            }
            Event::Blurred | Event::Enter => {
                self.handle_action(&Action::UpdateServerAddress, ctx);
            }
            _ => (),
        }
    }

    /// The address to save for what the user typed, which is the default for an empty field.
    /// Returns `None` for text that is not an address the app would connect to.
    fn parse_server_address(text: &str) -> Option<String> {
        let text = text.trim();
        if text.is_empty() {
            return Some(DEFAULT_REMOTE_ACCESS_SERVER_ADDRESS.to_owned());
        }
        agent_endpoint(text).is_ok().then(|| text.to_owned())
    }

    fn status_text(app: &AppContext) -> String {
        if !app.has_singleton_model::<HakoRemote>() {
            return "Off".to_owned();
        }
        match HakoRemote::as_ref(app).status() {
            RemoteAccessStatus::Off => "Off".to_owned(),
            RemoteAccessStatus::MissingKey => format!(
                "No access key found at {}",
                HakoRemote::key_path().display()
            ),
            RemoteAccessStatus::Misconfigured { error } => error.clone(),
            RemoteAccessStatus::Connecting { last_error: None } => "Connecting…".to_owned(),
            RemoteAccessStatus::Connecting {
                last_error: Some(error),
            } => format!("Not connected, trying again. {error}"),
            RemoteAccessStatus::Connected => "Connected".to_owned(),
        }
    }

    fn render_row(
        label: &'static str,
        value: Box<dyn Element>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_child(
                    Container::new(
                        Text::new_inline(
                            label,
                            appearance.ui_font_family(),
                            appearance.ui_font_size(),
                        )
                        .with_color(appearance.theme().active_ui_text_color().into())
                        .finish(),
                    )
                    .with_padding_right(8.5)
                    .finish(),
                )
                .with_child(value)
                .finish(),
        )
        .with_padding_bottom(6.)
        .finish()
    }

    fn render_server_address_editor(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let border_color = if self.is_server_address_valid {
            None
        } else {
            Some(crate::themes::theme::Fill::error().into())
        };
        appearance
            .ui_builder()
            .text_input(self.server_address_editor.clone())
            .with_style(UiComponentStyles {
                border_color,
                width: Some(SERVER_ADDRESS_EDITOR_WIDTH),
                padding: Some(Coords::uniform(5.)),
                background: Some(theme.surface_2().into()),
                ..Default::default()
            })
            .build()
            .finish()
    }
}

impl Entity for RemoteAccessView {
    type Event = ();
}

impl View for RemoteAccessView {
    fn ui_name() -> &'static str {
        "RemoteAccessView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let ui_builder = appearance.ui_builder();
        let theme = appearance.theme();
        let settings = RemoteAccessSettings::as_ref(app);

        let status = Text::new_inline(
            Self::status_text(app),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(theme.nonactive_ui_text_color().into())
        .finish();

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(render_body_item::<Action>(
                "Remote access".into(),
                None,
                LocalOnlyIconState::for_setting(
                    RemoteAccessEnabled::storage_key(),
                    RemoteAccessEnabled::sync_to_cloud(),
                    &mut self.local_only_icon_states.borrow_mut(),
                    app,
                ),
                ToggleState::Enabled,
                appearance,
                ui_builder
                    .switch(self.switch_state.clone())
                    .check(*settings.enabled)
                    .build()
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(Action::ToggleEnabled);
                    })
                    .finish(),
                Some(format!(
                    "Connects this computer to the remote access server, so that a browser signed in there can see its windows, tabs and terminals and open, switch to and close tabs. The access key is read from {}.{KEEP_AWAKE_NOTE}",
                    HakoRemote::key_path().display()
                )),
            ))
            .with_child(render_group(
                [
                    Self::render_row(
                        "Server address",
                        self.render_server_address_editor(appearance),
                        appearance,
                    ),
                    Self::render_row("Status", status, appearance),
                ],
                appearance,
            ))
            .finish()
    }
}

impl TypedActionView for RemoteAccessView {
    type Action = Action;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            Action::ToggleEnabled => {
                RemoteAccessSettings::handle(ctx).update(ctx, |settings, ctx| {
                    report_if_error!(settings.enabled.toggle_and_save_value(ctx));
                })
            }
            Action::UpdateServerAddress => {
                let text = self
                    .server_address_editor
                    .read(ctx, |editor, ctx| editor.buffer_text(ctx));
                let Some(server_address) = Self::parse_server_address(&text) else {
                    self.is_server_address_valid = false;
                    ctx.notify();
                    return;
                };
                self.is_server_address_valid = true;
                if *RemoteAccessSettings::as_ref(ctx).server_address.value() != server_address {
                    RemoteAccessSettings::handle(ctx).update(ctx, |settings, ctx| {
                        report_if_error!(settings.server_address.set_value(server_address, ctx));
                    });
                }
                ctx.notify();
            }
        }
    }
}
