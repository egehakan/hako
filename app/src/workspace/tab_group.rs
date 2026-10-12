//! Tab group data model. Gated at runtime by `FeatureFlag::GroupedTabs`.

use std::path::PathBuf;

use pathfinder_color::ColorU;
use uuid::Uuid;
use warp_core::ui::theme::{Fill as ThemeFill, WarpTheme};
use warpui::elements::DraggableState;

use crate::tab::SelectedTabColor;
use crate::workspace::tab_folders::{TabGroupIcon, default_folder_name};

/// Stable identity for a tab group.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TabGroupId(pub Uuid);

impl TabGroupId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TabGroupId {
    fn default() -> Self {
        Self::new()
    }
}

/// A named group of tabs in the vertical tabs panel.
/// Member tabs reference their group via `TabData::group_id`.
#[derive(Clone)]
pub struct TabGroup {
    pub id: TabGroupId,
    pub name: Option<String>,
    pub color: SelectedTabColor,
    pub collapsed: bool,
    pub draggable_state: DraggableState,
    /// True when this whole group is pinned to the front of the tab list.
    pub pinned: bool,
    /// The directory this group stands for. A group with a directory is a folder: it stays in
    /// the tab list without tabs, and new tabs opened from it start there.
    pub directory: Option<PathBuf>,
    /// A freely chosen color, which takes precedence over `color`.
    pub custom_color: Option<ColorU>,
    pub icon: Option<TabGroupIcon>,
    /// Whether `directory` was gone when last checked. Cached because rendering must not stat.
    pub directory_missing: bool,
}

impl TabGroup {
    /// Creates a new, untitled, expanded tab group with a fresh id.
    pub fn new() -> Self {
        Self {
            id: TabGroupId::new(),
            name: None,
            color: SelectedTabColor::default(),
            collapsed: false,
            draggable_state: Default::default(),
            pinned: false,
            directory: None,
            custom_color: None,
            icon: None,
            directory_missing: false,
        }
    }

    pub fn is_folder(&self) -> bool {
        self.directory.is_some()
    }

    /// The name shown in the tab list: the given name, else the folder's directory name.
    pub fn display_name(&self) -> String {
        match (&self.name, &self.directory) {
            (Some(name), _) => name.clone(),
            (None, Some(directory)) => default_folder_name(directory),
            (None, None) => "New Group".to_string(),
        }
    }

    /// The fill this group and its tabs are tinted with, if it has a color.
    pub fn fill(&self, theme: &WarpTheme) -> Option<ThemeFill> {
        match self.custom_color {
            Some(color) => Some(ThemeFill::Solid(color)),
            None => self
                .color
                .resolve(None)
                .map(|color| color.to_ansi_color(&theme.terminal_colors().normal).into()),
        }
    }

    /// Re-checks whether the folder's directory exists.
    pub fn refresh_directory_missing(&mut self) {
        self.directory_missing = self
            .directory
            .as_ref()
            .is_some_and(|directory| !directory.is_dir());
    }
}

impl Default for TabGroup {
    fn default() -> Self {
        Self::new()
    }
}
