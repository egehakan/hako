use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use pathfinder_color::ColorU;
use uuid::Uuid;

use super::*;

fn group_ids(count: u128) -> Vec<TabGroupId> {
    (1..=count)
        .map(|n| TabGroupId(Uuid::from_u128(n)))
        .collect()
}

fn existing(ids: &[TabGroupId]) -> HashSet<TabGroupId> {
    ids.iter().copied().collect()
}

#[test]
fn hex_colors_parse_in_long_and_short_form() {
    assert_eq!(
        parse_hex_color("#1E90FF"),
        Some(ColorU::new(0x1e, 0x90, 0xff, 255))
    );
    assert_eq!(
        parse_hex_color("  ff8800 "),
        Some(ColorU::new(0xff, 0x88, 0x00, 255))
    );
    assert_eq!(
        parse_hex_color("#abc"),
        Some(ColorU::new(0xaa, 0xbb, 0xcc, 255))
    );
}

#[test]
fn invalid_hex_colors_are_rejected() {
    for input in ["", "#", "#12", "#12345", "#1234567", "#gggggg", "red", "#é9"] {
        assert_eq!(parse_hex_color(input), None, "{input:?}");
    }
}

#[test]
fn hex_colors_round_trip() {
    let color = ColorU::new(0x0a, 0xb1, 0xfe, 255);
    assert_eq!(format_hex_color(color), "#0AB1FE");
    assert_eq!(parse_hex_color(&format_hex_color(color)), Some(color));
}

#[test]
fn hsl_colors_match_known_values() {
    assert_eq!(hsl_color(0., 1., 0.5), ColorU::new(255, 0, 0, 255));
    assert_eq!(hsl_color(120., 1., 0.5), ColorU::new(0, 255, 0, 255));
    assert_eq!(hsl_color(240., 1., 0.5), ColorU::new(0, 0, 255, 255));
    assert_eq!(hsl_color(0., 0., 1.), ColorU::new(255, 255, 255, 255));
    assert_eq!(hsl_color(200., 0., 0.), ColorU::new(0, 0, 0, 255));
}

#[test]
fn swatch_rows_are_rectangular_and_distinct() {
    let rows = color_swatch_rows();
    assert!(rows.iter().all(|row| row.len() == rows[0].len()));
    let all: Vec<ColorU> = rows.into_iter().flatten().collect();
    let distinct: HashSet<(u8, u8, u8)> = all.iter().map(|c| (c.r, c.g, c.b)).collect();
    assert_eq!(distinct.len(), all.len());
}

#[test]
fn icons_round_trip_through_their_serialized_form() {
    let material = TabGroupIcon::material("folder-api").expect("bundled icon");
    assert_eq!(material.serialize(), "material:folder-api");
    assert_eq!(
        TabGroupIcon::deserialize(&material.serialize()),
        Some(material)
    );

    let text = TabGroupIcon::text(" 🚀 ").expect("non-empty text");
    assert_eq!(text, TabGroupIcon::Text("🚀".to_string()));
    assert_eq!(TabGroupIcon::deserialize(&text.serialize()), Some(text));
}

#[test]
fn unknown_or_blank_icons_are_dropped() {
    assert_eq!(TabGroupIcon::material("no-such-icon"), None);
    assert_eq!(TabGroupIcon::deserialize("material:no-such-icon"), None);
    assert_eq!(TabGroupIcon::deserialize("text:   "), None);
    assert_eq!(TabGroupIcon::deserialize("folder-api"), None);
    assert_eq!(TabGroupIcon::text(""), None);
}

#[test]
fn text_icons_are_cut_to_a_few_characters() {
    assert_eq!(
        TabGroupIcon::text("abcdefghijkl"),
        Some(TabGroupIcon::Text("abcdefgh".to_string()))
    );
}

#[test]
fn every_bundled_icon_key_is_unique_and_has_an_asset() {
    let keys: HashSet<&str> = MATERIAL_GROUP_ICONS.iter().map(|(key, _)| *key).collect();
    assert_eq!(keys.len(), MATERIAL_GROUP_ICONS.len());
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
    for (key, path) in MATERIAL_GROUP_ICONS {
        assert!(assets.join(path).is_file(), "{key} has no asset at {path}");
    }
}

#[test]
fn a_directory_inside_a_repository_resolves_to_the_repository_root() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = normalized_directory(temp.path());
    let repo = root.join("repo");
    let nested = repo.join("crates").join("app");
    fs::create_dir_all(&nested).expect("nested dir");
    fs::create_dir(repo.join(".git")).expect("git dir");

    assert_eq!(folder_directory_for(&nested), repo);
    assert_eq!(folder_directory_for(&repo), repo);
}

#[test]
fn a_linked_worktree_resolves_to_its_own_root() {
    let temp = tempfile::tempdir().expect("temp dir");
    let worktree = normalized_directory(temp.path()).join("worktree");
    let nested = worktree.join("src");
    fs::create_dir_all(&nested).expect("nested dir");
    // A linked worktree has a `.git` file rather than a directory.
    fs::write(worktree.join(".git"), "gitdir: /elsewhere").expect("git file");

    assert_eq!(folder_directory_for(&nested), worktree);
}

#[test]
fn a_directory_outside_any_repository_resolves_to_itself() {
    let temp = tempfile::tempdir().expect("temp dir");
    let plain = normalized_directory(temp.path()).join("plain").join("deep");
    fs::create_dir_all(&plain).expect("plain dir");

    assert_eq!(folder_directory_for(&plain), plain);
}

#[test]
fn a_missing_directory_resolves_to_itself() {
    let missing = PathBuf::from("/nonexistent-tab-folder/child");
    assert_eq!(folder_directory_for(&missing), missing);
}

#[test]
fn folder_names_default_to_the_directory_name() {
    assert_eq!(default_folder_name(&PathBuf::from("/srv/projects/api")), "api");
    assert_eq!(default_folder_name(&PathBuf::from("/")), "/");
    if let Some(home) = dirs::home_dir() {
        assert_eq!(default_folder_name(&home), "Home");
        assert_eq!(display_directory(&home), "~");
        assert_eq!(display_directory(&home.join("code").join("api")), "~/code/api");
    }
    assert_eq!(display_directory(&PathBuf::from("/srv/api")), "/srv/api");
}

#[test]
fn groups_without_tabs_keep_their_slot_when_tabs_are_reordered() {
    let ids = group_ids(3);
    let (a, empty, b) = (ids[0], ids[1], ids[2]);

    // The tabs of `b` were moved in front of those of `a`.
    assert_eq!(
        normalized_group_order(&[a, empty, b], &[b, b, a], &existing(&ids)),
        vec![b, empty, a]
    );
}

#[test]
fn a_group_keeps_its_slot_when_it_loses_its_last_tab() {
    let ids = group_ids(3);
    let (a, b, c) = (ids[0], ids[1], ids[2]);

    assert_eq!(
        normalized_group_order(&[a, b, c], &[a, c], &existing(&ids)),
        vec![a, b, c]
    );
}

#[test]
fn unknown_groups_are_dropped_and_new_ones_appended() {
    let ids = group_ids(4);
    let (a, removed, with_tabs, without_tabs) = (ids[0], ids[1], ids[2], ids[3]);
    let existing = existing(&[a, with_tabs, without_tabs]);

    assert_eq!(
        normalized_group_order(&[removed, a], &[with_tabs, a], &existing),
        vec![with_tabs, a, without_tabs]
    );
}

#[test]
fn duplicate_ids_in_the_stored_order_are_ignored() {
    let ids = group_ids(2);
    let (a, b) = (ids[0], ids[1]);

    assert_eq!(
        normalized_group_order(&[a, b, a], &[], &existing(&ids)),
        vec![a, b]
    );
}

#[test]
fn the_tab_list_places_groups_without_tabs_between_their_neighbors() {
    let ids = group_ids(4);
    let (a, empty_middle, b, empty_last) = (ids[0], ids[1], ids[2], ids[3]);
    let tabs = [None, Some(a), Some(a), None, Some(b)];

    assert_eq!(
        tab_list_items(&tabs, &[a, empty_middle, b, empty_last]),
        vec![
            TabListItem::Tab(0),
            TabListItem::Group {
                id: a,
                members: 1..3
            },
            TabListItem::Tab(3),
            TabListItem::Group {
                id: empty_middle,
                members: 4..4
            },
            TabListItem::Group {
                id: b,
                members: 4..5
            },
            TabListItem::Group {
                id: empty_last,
                members: 5..5
            },
        ]
    );
}

#[test]
fn the_tab_list_shows_only_groups_without_tabs_when_there_are_no_grouped_tabs() {
    let ids = group_ids(2);

    assert_eq!(
        tab_list_items(&[None], &ids),
        vec![
            TabListItem::Tab(0),
            TabListItem::Group {
                id: ids[0],
                members: 1..1
            },
            TabListItem::Group {
                id: ids[1],
                members: 1..1
            },
        ]
    );
}

#[test]
fn the_first_tab_of_a_group_goes_in_front_of_the_next_group_with_tabs() {
    let ids = group_ids(4);
    let (a, empty, also_empty, b) = (ids[0], ids[1], ids[2], ids[3]);
    let tabs = [Some(a), None, Some(b), Some(b)];
    let order = [a, empty, also_empty, b];

    assert_eq!(insertion_index_for_empty_group(empty, &tabs, &order), 2);
    assert_eq!(insertion_index_for_empty_group(also_empty, &tabs, &order), 2);
    // Nothing with tabs follows, so the tab goes to the end of the list.
    assert_eq!(
        insertion_index_for_empty_group(empty, &tabs, &[a, b, empty]),
        4
    );

    // Filling the group there leaves the order of the groups as it was.
    let filled = [Some(a), None, Some(empty), Some(b), Some(b)];
    let groups_in_tab_order: Vec<TabGroupId> = filled.iter().flatten().copied().collect();
    assert_eq!(
        normalized_group_order(&order, &groups_in_tab_order, &existing(&ids)),
        order.to_vec()
    );
}

#[test]
fn a_folder_carries_its_looks_into_a_new_group() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut folder = TabFolder::new(temp.path().to_path_buf());
    folder.name = Some("API".to_string());
    folder.custom_color = parse_hex_color("#1E90FF");
    folder.icon = TabGroupIcon::material("folder-api");
    folder.collapsed = true;

    let group = folder.to_group();
    assert_eq!(group.directory.as_deref(), Some(temp.path()));
    assert!(group.collapsed);
    assert!(!group.directory_missing);
    assert_eq!(TabFolder::from_group(&group), Some(folder));
}

#[test]
fn a_group_without_a_directory_is_not_a_folder() {
    assert_eq!(TabFolder::from_group(&TabGroup::new()), None);
}

#[test]
fn a_folder_whose_directory_is_gone_is_marked_missing() {
    let group = TabFolder::new(PathBuf::from("/nonexistent-tab-folder")).to_group();
    assert!(group.directory_missing);
    assert_eq!(group.display_name(), "nonexistent-tab-folder");
}
