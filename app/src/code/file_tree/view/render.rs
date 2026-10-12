use warpui::Element;
use warpui::elements::{DraggableState, MouseStateHandle};

use super::FileTreeItem;
use crate::appearance::Appearance;
use crate::code::{file_icon_with_opacity, folder_icon_with_opacity};

/// How opaque an item that has been cut is drawn until it is pasted.
pub(super) const CUT_ITEM_OPACITY: f32 = 0.45;

impl FileTreeItem {
    pub(super) fn to_render_state(
        &self,
        is_expanded: Option<bool>,
        is_cut: bool,
        appearance: &Appearance,
    ) -> RenderState {
        let icon_opacity = if is_cut { CUT_ITEM_OPACITY } else { 1. };
        match self {
            FileTreeItem::File {
                metadata,
                mouse_state_handle,
                depth,
                draggable_state,
            } => {
                let display_name = metadata
                    .path
                    .file_name()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| String::from("File"));

                RenderState {
                    display_name,
                    icon: file_icon_with_opacity(metadata.path.as_str(), icon_opacity, appearance),
                    is_expanded,
                    depth: *depth,
                    mouse_state: mouse_state_handle.clone(),
                    draggable_state: draggable_state.clone(),
                    is_ignored: metadata.ignored,
                    is_cut,
                }
            }
            FileTreeItem::DirectoryHeader {
                directory,
                mouse_state_handle,
                depth,
                draggable_state,
            } => {
                let display_name = directory
                    .path
                    .file_name()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| String::from("Folder"));
                RenderState {
                    display_name,
                    icon: folder_icon_with_opacity(
                        directory.path.as_str(),
                        is_expanded.unwrap_or(false),
                        icon_opacity,
                    ),
                    is_expanded,
                    depth: *depth,
                    mouse_state: mouse_state_handle.clone(),
                    draggable_state: draggable_state.clone(),
                    is_ignored: directory.ignored,
                    is_cut,
                }
            }
        }
    }
}

pub(super) struct RenderState {
    pub display_name: String,
    pub icon: Box<dyn Element>,
    pub is_expanded: Option<bool>,
    pub depth: usize,
    pub mouse_state: MouseStateHandle,
    pub draggable_state: DraggableState,
    pub is_ignored: bool,
    pub is_cut: bool,
}
