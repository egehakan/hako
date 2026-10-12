//! Folders: tab groups that point at a directory and stay in the sidebar without tabs.

use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use pathfinder_color::ColorU;
use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{CacheOption, Element, Image};
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::tab::SelectedTabColor;
use crate::workspace::tab_group::{TabGroup, TabGroupId};

/// How many recently applied custom colors the customize dialog offers.
pub const MAX_RECENT_COLORS: usize = 8;

macro_rules! material_icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, concat!("bundled/svg/material_icon_theme/", $name, ".svg"))),*]
    };
}

/// Bundled Material icons a group can use, as `(key, asset path)`. The key is what is persisted.
pub const MATERIAL_GROUP_ICONS: &[(&str, &str)] = material_icons![
    "folder-project",
    "folder-src",
    "folder-app",
    "folder-api",
    "folder-server",
    "folder-client",
    "folder-core",
    "folder-lib",
    "folder-packages",
    "folder-components",
    "folder-ui",
    "folder-views",
    "folder-layout",
    "folder-config",
    "folder-environment",
    "folder-scripts",
    "folder-tools",
    "folder-utils",
    "folder-docs",
    "folder-test",
    "folder-benchmark",
    "folder-debug",
    "folder-log",
    "folder-database",
    "folder-migrations",
    "folder-docker",
    "folder-kubernetes",
    "folder-terraform",
    "folder-ci",
    "folder-git",
    "folder-github",
    "folder-gitlab",
    "folder-cloudflare",
    "folder-vercel",
    "folder-supabase",
    "folder-serverless",
    "folder-rust",
    "folder-go",
    "folder-python",
    "folder-node",
    "folder-javascript",
    "folder-typescript",
    "folder-java",
    "folder-php",
    "folder-next",
    "folder-svelte",
    "folder-android",
    "folder-ios",
    "folder-macos",
    "folder-linux",
    "folder-windows",
    "folder-mobile",
    "folder-desktop",
    "folder-images",
    "folder-video",
    "folder-audio",
    "folder-font",
    "folder-theme",
    "folder-mail",
    "folder-download",
    "folder-upload",
    "folder-archive",
    "folder-backup",
    "folder-temp",
    "folder-trash",
    "folder-secure",
    "folder-keys",
    "folder-private",
    "folder-public",
    "folder-shared",
    "folder-robot",
    "folder-claude",
    "folder-prompts",
    "folder-sandbox",
    "folder-admin",
    "folder-other",
    "console",
    "rust",
    "go",
    "python",
    "typescript",
    "javascript",
    "react",
    "vue",
    "svelte",
    "next",
    "nodejs",
    "docker",
    "database",
    "terraform",
    "git",
    "swift",
    "kotlin",
    "java",
    "ruby",
    "php",
    "csharp",
    "cpp",
    "c",
    "zig",
    "elixir",
    "haskell",
    "lua",
    "html",
    "css",
    "json",
    "yaml",
    "markdown",
    "settings",
    "key",
    "agent",
    "claude",
    "figma",
    "supabase",
    "vercel",
];

/// The icon shown in a group's header in place of the default one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabGroupIcon {
    /// A key of [`MATERIAL_GROUP_ICONS`].
    Material(&'static str),
    /// A short piece of text, typically one emoji.
    Text(String),
}

impl TabGroupIcon {
    const MATERIAL_PREFIX: &'static str = "material:";
    const TEXT_PREFIX: &'static str = "text:";
    /// Longest text icon, in characters; an emoji can be several code points.
    const MAX_TEXT_CHARS: usize = 8;

    /// The icon for a key of [`MATERIAL_GROUP_ICONS`], if the key is known.
    pub fn material(key: &str) -> Option<Self> {
        MATERIAL_GROUP_ICONS
            .iter()
            .find(|(candidate, _)| *candidate == key)
            .map(|(key, _)| Self::Material(*key))
    }

    /// A text icon from user input, or `None` when the input is blank.
    pub fn text(input: &str) -> Option<Self> {
        let text: String = input.trim().chars().take(Self::MAX_TEXT_CHARS).collect();
        (!text.is_empty()).then_some(Self::Text(text))
    }

    pub fn serialize(&self) -> String {
        match self {
            Self::Material(key) => format!("{}{key}", Self::MATERIAL_PREFIX),
            Self::Text(text) => format!("{}{text}", Self::TEXT_PREFIX),
        }
    }

    /// Parses [`Self::serialize`] output. Unknown icons are dropped rather than failing a restore.
    pub fn deserialize(serialized: &str) -> Option<Self> {
        if let Some(key) = serialized.strip_prefix(Self::MATERIAL_PREFIX) {
            Self::material(key)
        } else {
            serialized
                .strip_prefix(Self::TEXT_PREFIX)
                .and_then(Self::text)
        }
    }

    /// The bundled image for a Material icon; `None` for a text icon.
    pub fn material_image(&self) -> Option<Box<dyn Element>> {
        let Self::Material(key) = self else {
            return None;
        };
        let (_, path) = MATERIAL_GROUP_ICONS
            .iter()
            .find(|(candidate, _)| candidate == key)?;
        Some(Image::new(AssetSource::Bundled { path: *path }, CacheOption::BySize).finish())
    }
}

/// Parses `#RGB`, `#RRGGBB` or the same without the `#`, ignoring case and surrounding space.
pub fn parse_hex_color(input: &str) -> Option<ColorU> {
    let digits = input.trim().trim_start_matches('#');
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |index: usize, width: usize| {
        let value = u8::from_str_radix(&digits[index * width..(index + 1) * width], 16).ok()?;
        // A single digit stands for the doubled digit: `f` is `ff`.
        Some(if width == 1 { value * 17 } else { value })
    };
    let width = match digits.len() {
        3 => 1,
        6 => 2,
        _ => return None,
    };
    Some(ColorU::new(
        channel(0, width)?,
        channel(1, width)?,
        channel(2, width)?,
        255,
    ))
}

/// Formats a color as `#RRGGBB`. Alpha is not kept: group colors are always opaque.
pub fn format_hex_color(color: ColorU) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b)
}

/// Converts HSL (hue in degrees, saturation and lightness in `0..=1`) to an opaque color.
pub fn hsl_color(hue: f32, saturation: f32, lightness: f32) -> ColorU {
    let chroma = (1. - (2. * lightness - 1.).abs()) * saturation;
    let sector = hue.rem_euclid(360.) / 60.;
    let secondary = chroma * (1. - (sector % 2. - 1.).abs());
    let (r, g, b) = match sector as u32 {
        0 => (chroma, secondary, 0.),
        1 => (secondary, chroma, 0.),
        2 => (0., chroma, secondary),
        3 => (0., secondary, chroma),
        4 => (secondary, 0., chroma),
        _ => (chroma, 0., secondary),
    };
    let offset = lightness - chroma / 2.;
    let channel = |value: f32| ((value + offset) * 255.).round().clamp(0., 255.) as u8;
    ColorU::new(channel(r), channel(g), channel(b), 255)
}

/// The swatches offered by the customize dialog: one row per shade, one column per hue, with a
/// leading gray column.
pub fn color_swatch_rows() -> Vec<Vec<ColorU>> {
    const HUES: [f32; 12] = [
        0., 20., 40., 55., 90., 140., 170., 195., 215., 250., 280., 325.,
    ];
    const SHADES: [(f32, f32); 5] = [
        (0.85, 0.78),
        (0.80, 0.64),
        (0.75, 0.50),
        (0.70, 0.38),
        (0.60, 0.26),
    ];
    SHADES
        .iter()
        .enumerate()
        .map(|(row, (saturation, lightness))| {
            let gray = hsl_color(0., 0., 0.85 - row as f32 * 0.16);
            std::iter::once(gray)
                .chain(
                    HUES.iter()
                        .map(|hue| hsl_color(*hue, *saturation, *lightness)),
                )
                .collect()
        })
        .collect()
}

/// The directory a folder is made for when a terminal starts in `start`: the root of the git
/// repository containing it, or `start` itself outside a repository.
pub fn folder_directory_for(start: &Path) -> PathBuf {
    let start = normalized_directory(start);
    start
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
        .unwrap_or(start)
}

/// Resolves symlinks when the directory exists, so one directory never yields two folders.
pub fn normalized_directory(directory: &Path) -> PathBuf {
    dunce::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf())
}

/// The name a folder shows until it is renamed.
pub fn default_folder_name(directory: &Path) -> String {
    if dirs::home_dir().is_some_and(|home| home == directory) {
        return "Home".to_string();
    }
    directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| directory.to_string_lossy().into_owned())
}

/// The directory as shown under a folder's name, with the home directory shortened to `~`.
pub fn display_directory(directory: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(relative) = directory.strip_prefix(&home)
    {
        return if relative.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", relative.to_string_lossy())
        };
    }
    directory.to_string_lossy().into_owned()
}

/// Returns `stored` brought in line with the tab list: groups that hold tabs take the slots of
/// such groups in the order their tabs appear, groups without tabs keep their slot, and groups
/// missing from `stored` are appended. Ids that are not in `existing` are dropped.
pub fn normalized_group_order(
    stored: &[TabGroupId],
    groups_in_tab_order: &[TabGroupId],
    existing: &HashSet<TabGroupId>,
) -> Vec<TabGroupId> {
    let mut seen = HashSet::new();
    let with_tabs: Vec<TabGroupId> = groups_in_tab_order
        .iter()
        .copied()
        .filter(|id| existing.contains(id) && seen.insert(*id))
        .collect();
    let has_tabs: HashSet<TabGroupId> = with_tabs.iter().copied().collect();

    let mut seen = HashSet::new();
    let mut order: Vec<TabGroupId> = stored
        .iter()
        .copied()
        .filter(|id| existing.contains(id) && seen.insert(*id))
        .collect();
    order.extend(with_tabs.iter().copied().filter(|id| seen.insert(*id)));
    // Ids are random, so sorting only makes the order of never-placed empty groups stable.
    let mut unplaced: Vec<TabGroupId> = existing
        .iter()
        .copied()
        .filter(|id| !seen.contains(id))
        .collect();
    unplaced.sort_by_key(|id| id.0);
    order.extend(unplaced);

    let mut next_with_tabs = with_tabs.into_iter();
    for slot in &mut order {
        if has_tabs.contains(slot)
            && let Some(id) = next_with_tabs.next()
        {
            *slot = id;
        }
    }
    order
}

/// One top-level entry of the tab list as the sidebar shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabListItem {
    /// A tab outside any group, by tab index.
    Tab(usize),
    /// A group with the indices of its tabs, which is empty for a group without tabs.
    Group {
        id: TabGroupId,
        members: Range<usize>,
    },
}

/// Lays out the tab list: tabs and groups in tab order, with every group that has no tabs placed
/// before the next group that follows it in `order` (or at the end).
pub fn tab_list_items(tab_groups: &[Option<TabGroupId>], order: &[TabGroupId]) -> Vec<TabListItem> {
    let has_tabs: HashSet<TabGroupId> = tab_groups.iter().flatten().copied().collect();
    let mut emitted = HashSet::new();
    let mut items = Vec::new();
    let mut index = 0;
    while index < tab_groups.len() {
        let Some(group_id) = tab_groups[index] else {
            items.push(TabListItem::Tab(index));
            index += 1;
            continue;
        };
        let run = tab_groups[index..]
            .iter()
            .take_while(|id| **id == Some(group_id))
            .count();
        if emitted.insert(group_id) {
            let slot = order.iter().position(|id| *id == group_id);
            for empty in order[..slot.unwrap_or(0)]
                .iter()
                .filter(|id| !has_tabs.contains(id))
            {
                if emitted.insert(*empty) {
                    items.push(TabListItem::Group {
                        id: *empty,
                        members: index..index,
                    });
                }
            }
        }
        items.push(TabListItem::Group {
            id: group_id,
            members: index..index + run,
        });
        index += run;
    }
    for empty in order.iter().filter(|id| !has_tabs.contains(id)) {
        if emitted.insert(*empty) {
            items.push(TabListItem::Group {
                id: *empty,
                members: tab_groups.len()..tab_groups.len(),
            });
        }
    }
    items
}

/// The tab index at which the first tab of a group without tabs has to be inserted for the group
/// to keep its place: in front of the next group in `order` that holds tabs, else at the end.
pub fn insertion_index_for_empty_group(
    group_id: TabGroupId,
    tab_groups: &[Option<TabGroupId>],
    order: &[TabGroupId],
) -> usize {
    let Some(slot) = order.iter().position(|id| *id == group_id) else {
        return tab_groups.len();
    };
    order[slot + 1..]
        .iter()
        .find_map(|next| tab_groups.iter().position(|id| *id == Some(*next)))
        .unwrap_or(tab_groups.len())
}

/// A folder as the whole app knows it, independent of any window.
#[derive(Clone, Debug, PartialEq)]
pub struct TabFolder {
    pub directory: PathBuf,
    pub name: Option<String>,
    pub color: SelectedTabColor,
    pub custom_color: Option<ColorU>,
    pub icon: Option<TabGroupIcon>,
    pub collapsed: bool,
}

impl TabFolder {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            name: None,
            color: SelectedTabColor::Unset,
            custom_color: None,
            icon: None,
            collapsed: false,
        }
    }

    /// The folder as a window's group describes it.
    pub fn from_group(group: &TabGroup) -> Option<Self> {
        Some(Self {
            directory: group.directory.clone()?,
            name: group.name.clone(),
            color: group.color,
            custom_color: group.custom_color,
            icon: group.icon.clone(),
            collapsed: group.collapsed,
        })
    }

    /// A new group for a window that does not show this folder yet.
    pub fn to_group(&self) -> TabGroup {
        let mut group = TabGroup::new();
        group.directory = Some(self.directory.clone());
        group.collapsed = self.collapsed;
        self.apply_appearance(&mut group);
        group.refresh_directory_missing();
        group
    }

    /// Copies what all windows share about a folder; where it sits and whether it is collapsed
    /// stay per window.
    pub fn apply_appearance(&self, group: &mut TabGroup) {
        group.name = self.name.clone();
        group.color = self.color;
        group.custom_color = self.custom_color;
        group.icon = self.icon.clone();
    }
}

pub enum TabFoldersEvent {
    Changed,
}

/// The folders shown in every window's tab list. Windows come and go; this is what keeps a folder
/// in place after its last tab, or the window it was in, is closed.
#[derive(Default)]
pub struct TabFolders {
    folders: Vec<TabFolder>,
    recent_colors: Vec<ColorU>,
}

impl TabFolders {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn folders(&self) -> &[TabFolder] {
        &self.folders
    }

    pub fn folder(&self, directory: &Path) -> Option<&TabFolder> {
        self.folders
            .iter()
            .find(|folder| folder.directory == directory)
    }

    pub fn recent_colors(&self) -> &[ColorU] {
        &self.recent_colors
    }

    /// Replaces all folders, keeping the first of any that share a directory.
    pub fn restore(&mut self, folders: Vec<TabFolder>, ctx: &mut ModelContext<Self>) {
        let mut seen = HashSet::new();
        self.folders = folders
            .into_iter()
            .filter(|folder| seen.insert(folder.directory.clone()))
            .collect();
        let colors: Vec<ColorU> = self
            .folders
            .iter()
            .filter_map(|folder| folder.custom_color)
            .collect();
        for color in colors.into_iter().rev() {
            self.note_recent_color(color);
        }
        ctx.emit(TabFoldersEvent::Changed);
    }

    /// Adds the folder, or updates the one with the same directory in place.
    pub fn upsert(&mut self, folder: TabFolder, ctx: &mut ModelContext<Self>) {
        if let Some(custom_color) = folder.custom_color {
            self.note_recent_color(custom_color);
        }
        match self
            .folders
            .iter_mut()
            .find(|existing| existing.directory == folder.directory)
        {
            Some(existing) if *existing == folder => return,
            Some(existing) => *existing = folder,
            None => self.folders.push(folder),
        }
        ctx.emit(TabFoldersEvent::Changed);
    }

    pub fn remove(&mut self, directory: &Path, ctx: &mut ModelContext<Self>) {
        let count = self.folders.len();
        self.folders.retain(|folder| folder.directory != directory);
        if self.folders.len() != count {
            ctx.emit(TabFoldersEvent::Changed);
        }
    }

    /// Reorders the folders to follow `directories`; folders not listed keep their relative order
    /// after the listed ones. Windows keep their own order, so no event is emitted.
    pub fn set_order(&mut self, directories: &[PathBuf]) {
        self.folders.sort_by_key(|folder| {
            directories
                .iter()
                .position(|directory| *directory == folder.directory)
                .unwrap_or(usize::MAX)
        });
    }

    fn note_recent_color(&mut self, color: ColorU) {
        self.recent_colors.retain(|recent| *recent != color);
        self.recent_colors.insert(0, color);
        self.recent_colors.truncate(MAX_RECENT_COLORS);
    }
}

impl Entity for TabFolders {
    type Event = TabFoldersEvent;
}

impl SingletonEntity for TabFolders {}

#[cfg(test)]
#[path = "tab_folders_tests.rs"]
mod tests;
