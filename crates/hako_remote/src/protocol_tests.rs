use serde_json::json;

use super::*;

#[test]
fn decodes_a_request() {
    let frame =
        Frame::decode(r#"{ "t": "req", "id": "7", "method": "workspace.list", "params": {} }"#)
            .unwrap();

    assert_eq!(
        frame,
        Frame::Request {
            id: "7".to_owned(),
            method: "workspace.list".to_owned(),
            params: json!({}),
        }
    );
}

#[test]
fn gives_a_request_without_params_an_empty_object() {
    let missing = Frame::decode(r#"{ "t": "req", "id": "c1:7", "method": "app.info" }"#).unwrap();
    let null =
        Frame::decode(r#"{ "t": "req", "id": "c1:7", "method": "app.info", "params": null }"#)
            .unwrap();

    let expected = Frame::Request {
        id: "c1:7".to_owned(),
        method: "app.info".to_owned(),
        params: json!({}),
    };
    assert_eq!(missing, expected);
    assert_eq!(null, expected);
}

#[test]
fn decodes_a_notification() {
    let frame = Frame::decode(
        r#"{ "t": "ntf", "method": "term.input", "params": { "session_id": "s1", "data": "ls\r" } }"#,
    )
    .unwrap();

    assert_eq!(
        frame,
        Frame::Notification {
            method: "term.input".to_owned(),
            params: json!({ "session_id": "s1", "data": "ls\r" }),
        }
    );
}

#[test]
fn encodes_a_successful_response() {
    let frame = Frame::Response(Response::new("7".to_owned(), Ok(json!({}))));

    assert_eq!(
        serde_json::to_value(&frame).unwrap(),
        json!({ "t": "res", "id": "7", "ok": true, "result": {} })
    );
}

#[test]
fn encodes_a_failed_response() {
    let frame = Frame::Response(Response::new(
        "7".to_owned(),
        Err(RpcError::not_found("No such tab")),
    ));

    assert_eq!(
        serde_json::to_value(&frame).unwrap(),
        json!({
            "t": "res",
            "id": "7",
            "ok": false,
            "error": { "code": "not_found", "message": "No such tab" }
        })
    );
}

#[test]
fn encodes_an_event() {
    let frame = Frame::event(EVENT_WORKSPACE_CHANGED, json!({}));

    assert_eq!(
        serde_json::to_value(&frame).unwrap(),
        json!({ "t": "evt", "event": "workspace.changed", "data": {} })
    );
}

#[test]
fn rejects_a_frame_of_an_unknown_type() {
    assert!(Frame::decode(r#"{ "t": "hello" }"#).is_err());
}

#[test]
fn names_every_error_code_as_the_protocol_does() {
    let codes = [
        ErrorCode::AgentOffline,
        ErrorCode::Timeout,
        ErrorCode::BadRequest,
        ErrorCode::NotFound,
        ErrorCode::NotSupported,
        ErrorCode::Failed,
        ErrorCode::NotADirectory,
        ErrorCode::IsADirectory,
        ErrorCode::PermissionDenied,
        ErrorCode::Conflict,
        ErrorCode::TooLarge,
        ErrorCode::GitFailed,
        ErrorCode::Busy,
    ];

    assert_eq!(
        serde_json::to_value(codes).unwrap(),
        json!([
            "agent_offline",
            "timeout",
            "bad_request",
            "not_found",
            "not_supported",
            "failed",
            "not_a_directory",
            "is_a_directory",
            "permission_denied",
            "conflict",
            "too_large",
            "git_failed",
            "busy"
        ])
    );
}

#[test]
fn knows_the_methods_of_this_version_only() {
    assert_eq!(Method::from_name("app.info"), Some(Method::AppInfo));
    assert_eq!(
        Method::from_name("workspace.list"),
        Some(Method::WorkspaceList)
    );
    assert_eq!(Method::from_name("tab.create"), Some(Method::TabCreate));
    assert_eq!(Method::from_name("tab.activate"), Some(Method::TabActivate));
    assert_eq!(Method::from_name("tab.close"), Some(Method::TabClose));
    assert_eq!(Method::from_name("tab.rename"), Some(Method::TabRename));
    assert_eq!(
        Method::from_name("folder.create"),
        Some(Method::FolderCreate)
    );
    assert_eq!(
        Method::from_name("folder.rename"),
        Some(Method::FolderRename)
    );
    assert_eq!(
        Method::from_name("term.subscribe"),
        Some(Method::TermSubscribe)
    );
    assert_eq!(
        Method::from_name("term.unsubscribe"),
        Some(Method::TermUnsubscribe)
    );
    assert_eq!(Method::from_name("term.resize"), Some(Method::TermResize));
    assert_eq!(
        Method::from_name("term.release_size"),
        Some(Method::TermReleaseSize)
    );
    assert_eq!(Method::from_name("theme.get"), Some(Method::ThemeGet));
    // Input is a notification, which has no answer.
    assert_eq!(Method::from_name("term.input"), None);
    assert_eq!(Method::from_name("fs.roots"), Some(Method::FsRoots));
    assert_eq!(Method::from_name("fs.list"), Some(Method::FsList));
    assert_eq!(Method::from_name("fs.read"), Some(Method::FsRead));
    assert_eq!(Method::from_name("fs.write"), Some(Method::FsWrite));
    assert_eq!(Method::from_name("fs.mkdir"), Some(Method::FsMkdir));
    assert_eq!(Method::from_name("fs.stat"), Some(Method::FsStat));
    assert_eq!(
        Method::from_name("fs.create_file"),
        Some(Method::FsCreateFile)
    );
    assert_eq!(Method::from_name("fs.rename"), Some(Method::FsRename));
    assert_eq!(Method::from_name("fs.copy"), Some(Method::FsCopy));
    assert_eq!(Method::from_name("fs.move"), Some(Method::FsMove));
    assert_eq!(Method::from_name("fs.trash"), Some(Method::FsTrash));
    assert_eq!(Method::from_name("fs.archive"), Some(Method::FsArchive));
    assert_eq!(Method::from_name("fs.extract"), Some(Method::FsExtract));
    assert_eq!(
        Method::from_name("fs.transfer_info"),
        Some(Method::FsTransferInfo)
    );
    assert_eq!(Method::from_name("fs.download"), Some(Method::FsDownload));
    assert_eq!(
        Method::from_name("fs.upload_begin"),
        Some(Method::FsUploadBegin)
    );
    assert_eq!(
        Method::from_name("fs.upload_chunk"),
        Some(Method::FsUploadChunk)
    );
    assert_eq!(
        Method::from_name("fs.upload_end"),
        Some(Method::FsUploadEnd)
    );
    assert_eq!(
        Method::from_name("fs.upload_cancel"),
        Some(Method::FsUploadCancel)
    );
    assert_eq!(Method::from_name("git.repos"), Some(Method::GitRepos));
    assert_eq!(Method::from_name("git.status"), Some(Method::GitStatus));
    assert_eq!(Method::from_name("git.diff"), Some(Method::GitDiff));
    assert_eq!(Method::from_name("git.commit"), Some(Method::GitCommit));
    assert_eq!(Method::from_name("git.push"), Some(Method::GitPush));
    assert_eq!(Method::from_name("git.pull"), Some(Method::GitPull));
    assert_eq!(Method::from_name("git.discard"), Some(Method::GitDiscard));
    assert_eq!(Method::from_name("git.branches"), Some(Method::GitBranches));
    assert_eq!(Method::from_name("git.switch"), Some(Method::GitSwitch));
    assert_eq!(Method::from_name("git.log"), Some(Method::GitLog));
    assert_eq!(Method::from_name("git.show"), Some(Method::GitShow));
    assert_eq!(Method::from_name("git.merge"), Some(Method::GitMerge));
    assert_eq!(
        Method::from_name("git.merge_abort"),
        Some(Method::GitMergeAbort)
    );
    assert_eq!(Method::from_name("git.fetch"), None);
}

#[test]
fn encodes_a_listing_as_the_protocol_names_its_fields() {
    let list = FsList {
        entries: vec![
            FsEntry {
                name: "docs".to_owned(),
                kind: FsEntryKind::Symlink,
                target: Some(FsLinkTarget::Dir),
                size: 0,
                mtime: 1_760_000_000_123,
            },
            FsEntry {
                name: "a.txt".to_owned(),
                kind: FsEntryKind::File,
                target: None,
                size: 5,
                mtime: -1,
            },
        ],
        truncated: false,
    };

    assert_eq!(
        serde_json::to_value(list).unwrap(),
        json!({
            "entries": [
                { "name": "docs", "kind": "symlink", "target": "dir", "size": 0, "mtime": 1_760_000_000_123_i64 },
                { "name": "a.txt", "kind": "file", "target": null, "size": 5, "mtime": -1 },
            ],
            "truncated": false,
        })
    );
}

#[test]
fn encodes_each_kind_of_file_contents() {
    let text = FsRead::Text {
        content: "hi".to_owned(),
        mtime: 7,
        size: 2,
    };
    let image = FsRead::Image {
        mime: "image/png".to_owned(),
        data: "AQID".to_owned(),
        mtime: 7,
        size: 3,
    };
    let too_large = FsRead::TooLarge { size: 9, limit: 8 };

    assert_eq!(
        serde_json::to_value(text).unwrap(),
        json!({ "kind": "text", "content": "hi", "mtime": 7, "size": 2 })
    );
    assert_eq!(
        serde_json::to_value(image).unwrap(),
        json!({ "kind": "image", "mime": "image/png", "data": "AQID", "mtime": 7, "size": 3 })
    );
    assert_eq!(
        serde_json::to_value(FsRead::Binary { size: 4 }).unwrap(),
        json!({ "kind": "binary", "size": 4 })
    );
    assert_eq!(
        serde_json::to_value(too_large).unwrap(),
        json!({ "kind": "too_large", "size": 9, "limit": 8 })
    );
}

#[test]
fn requires_the_expected_mtime_of_a_write_to_be_given() {
    let params = |value| serde_json::from_value::<FsWriteParams>(value);

    let checked = params(json!({ "path": "/a", "content": "x", "expected_mtime": 7 })).unwrap();
    let unchecked =
        params(json!({ "path": "/a", "content": "x", "expected_mtime": null })).unwrap();

    assert_eq!(checked.expected_mtime, Some(7));
    assert_eq!(unchecked.expected_mtime, None);
    assert!(params(json!({ "path": "/a", "content": "x" })).is_err());
    assert!(params(json!({ "path": "/a", "content": 1, "expected_mtime": null })).is_err());
    assert!(params(json!({ "path": "/a", "content": "x", "expected_mtime": "7" })).is_err());
}

#[test]
fn encodes_the_roots_with_their_sources() {
    let roots = FsRoots {
        home: "/Users/ege".to_owned(),
        roots: vec![FsRoot {
            path: "/".to_owned(),
            name: "/".to_owned(),
            source: FsRootSource::Terminal,
        }],
    };

    assert_eq!(
        serde_json::to_value(roots).unwrap(),
        json!({
            "home": "/Users/ege",
            "roots": [{ "path": "/", "name": "/", "source": "terminal" }],
        })
    );
}

#[test]
fn encodes_app_info() {
    let info = AppInfo {
        name: "Ege's MacBook Pro".to_owned(),
        version: "v0.2026.10.13".to_owned(),
        hostname: "eges-mbp.local".to_owned(),
        protocol: PROTOCOL_VERSION,
    };

    assert_eq!(
        serde_json::to_value(&info).unwrap(),
        json!({
            "name": "Ege's MacBook Pro",
            "version": "v0.2026.10.13",
            "hostname": "eges-mbp.local",
            "protocol": 1
        })
    );
}

#[test]
fn encodes_a_workspace_with_nulls_for_what_is_absent() {
    let workspace = Workspace {
        windows: vec![Window {
            id: "1".to_owned(),
            active: true,
            folders: vec![Folder {
                id: "f1".to_owned(),
                name: "warp".to_owned(),
                directory: "/Users/ege/warp".to_owned(),
                color: Some("#E06C75".to_owned()),
                icon: Some("material:folder-src".to_owned()),
            }],
            tabs: vec![
                Tab {
                    id: "10".to_owned(),
                    title: "codex".to_owned(),
                    active: true,
                    folder_id: Some("f1".to_owned()),
                    panes: vec![Pane {
                        id: "11".to_owned(),
                        kind: PaneKind::Terminal,
                        title: "codex".to_owned(),
                        focused: true,
                        session_id: Some("s1".to_owned()),
                        cwd: Some("/Users/ege/warp".to_owned()),
                        agent: Some(AgentState {
                            name: "Codex".to_owned(),
                            status: AgentStatus::Waiting,
                            message: Some("Allow Bash?".to_owned()),
                        }),
                    }],
                },
                Tab {
                    id: "20".to_owned(),
                    title: "README.md".to_owned(),
                    active: false,
                    folder_id: None,
                    panes: vec![Pane {
                        id: "21".to_owned(),
                        kind: PaneKind::File,
                        title: "README.md".to_owned(),
                        focused: true,
                        session_id: None,
                        cwd: None,
                        agent: None,
                    }],
                },
            ],
        }],
    };

    assert_eq!(
        serde_json::to_value(&workspace).unwrap(),
        json!({
            "windows": [{
                "id": "1",
                "active": true,
                "folders": [{
                    "id": "f1",
                    "name": "warp",
                    "directory": "/Users/ege/warp",
                    "color": "#E06C75",
                    "icon": "material:folder-src"
                }],
                "tabs": [
                    {
                        "id": "10",
                        "title": "codex",
                        "active": true,
                        "folder_id": "f1",
                        "panes": [{
                            "id": "11",
                            "kind": "terminal",
                            "title": "codex",
                            "focused": true,
                            "session_id": "s1",
                            "cwd": "/Users/ege/warp",
                            "agent": {
                                "name": "Codex",
                                "status": "waiting",
                                "message": "Allow Bash?"
                            }
                        }]
                    },
                    {
                        "id": "20",
                        "title": "README.md",
                        "active": false,
                        "folder_id": null,
                        "panes": [{
                            "id": "21",
                            "kind": "file",
                            "title": "README.md",
                            "focused": true,
                            "session_id": null,
                            "cwd": null,
                            "agent": null
                        }]
                    }
                ]
            }]
        })
    );
}

#[test]
fn reads_tab_create_params_with_any_field_left_out() {
    let none: TabCreateParams = serde_json::from_value(json!({})).unwrap();
    let folder: TabCreateParams =
        serde_json::from_value(json!({ "folder_id": "f1", "window_id": "1" })).unwrap();

    assert_eq!(none, TabCreateParams::default());
    assert_eq!(
        folder,
        TabCreateParams {
            window_id: Some("1".to_owned()),
            folder_id: Some("f1".to_owned()),
            directory: None,
        }
    );
}

#[test]
fn reads_folder_create_params_with_or_without_a_window() {
    let frontmost: FolderCreateParams =
        serde_json::from_value(json!({ "directory": "/work/site" })).unwrap();
    let named: FolderCreateParams =
        serde_json::from_value(json!({ "directory": "/work/site", "window_id": "3" })).unwrap();

    assert_eq!(
        frontmost,
        FolderCreateParams {
            directory: "/work/site".to_owned(),
            window_id: None,
        }
    );
    assert_eq!(named.window_id.as_deref(), Some("3"));
    assert!(serde_json::from_value::<FolderCreateParams>(json!({})).is_err());
    assert_eq!(
        json!(FolderCreateResult {
            folder_id: "12".to_owned()
        }),
        json!({ "folder_id": "12" })
    );
}

#[test]
fn encodes_a_subscription_as_the_protocol_names_its_fields() {
    let result = TermSubscribeResult {
        cols: 80,
        rows: 24,
        seq: 4096,
        snapshot: "aGk=".to_owned(),
    };

    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({ "cols": 80, "rows": 24, "seq": 4096, "snapshot": "aGk=" })
    );
}

#[test]
fn encodes_a_repository_and_a_file_as_the_protocol_names_their_fields() {
    let repo = GitRepo {
        path: "/code/site-redesign".to_owned(),
        name: "site-redesign".to_owned(),
        root: "/code".to_owned(),
        is_worktree: true,
        main_path: Some("/code/site".to_owned()),
        branch: None,
        head: Some("0a1b".to_owned()),
        upstream: None,
        ahead: 0,
        behind: 2,
        changed: 3,
        operation: Some(GitOperation::CherryPick),
    };
    let file = GitFile {
        path: "b.txt".to_owned(),
        old_path: Some("a.txt".to_owned()),
        status: GitFileStatus::Renamed,
        additions: None,
        deletions: None,
        binary: true,
    };

    assert_eq!(
        serde_json::to_value(repo).unwrap(),
        json!({
            "path": "/code/site-redesign",
            "name": "site-redesign",
            "root": "/code",
            "is_worktree": true,
            "main_path": "/code/site",
            "branch": null,
            "head": "0a1b",
            "upstream": null,
            "ahead": 0,
            "behind": 2,
            "changed": 3,
            "operation": "cherry-pick"
        })
    );
    assert_eq!(
        serde_json::to_value(file).unwrap(),
        json!({
            "path": "b.txt",
            "old_path": "a.txt",
            "status": "renamed",
            "additions": null,
            "deletions": null,
            "binary": true
        })
    );
}

#[test]
fn encodes_each_kind_of_diff() {
    let binary = GitDiff::Binary {
        old_size: None,
        new_size: Some(4),
    };

    assert_eq!(
        serde_json::to_value(GitDiff::Text {
            diff: String::new()
        })
        .unwrap(),
        json!({ "kind": "text", "diff": "" })
    );
    assert_eq!(
        serde_json::to_value(binary).unwrap(),
        json!({ "kind": "binary", "old_size": null, "new_size": 4 })
    );
    assert_eq!(
        serde_json::to_value(GitDiff::TooLarge { limit: 1_048_576 }).unwrap(),
        json!({ "kind": "too_large", "limit": 1_048_576 })
    );
}

#[test]
fn reads_the_paths_of_a_discard_when_they_are_null_or_left_out() {
    let params = |value| serde_json::from_value::<GitDiscardParams>(value);

    let everything = params(json!({ "repo": "/r" })).unwrap();
    let nulls = params(json!({ "repo": "/r", "path": null, "old_path": null })).unwrap();
    let one = params(json!({ "repo": "/r", "path": "a.txt" })).unwrap();

    assert_eq!(everything.path, None);
    assert_eq!(nulls, everything);
    assert_eq!(one.path.as_deref(), Some("a.txt"));
    assert!(params(json!({ "path": "a.txt" })).is_err());
    assert!(params(json!({ "repo": 1 })).is_err());
}

#[test]
fn encodes_a_branch_and_a_commit_as_the_protocol_names_their_fields() {
    let branch = GitBranch {
        name: "origin/main".to_owned(),
        kind: GitBranchKind::Remote,
        tip: "0a1b".to_owned(),
        time: 1_700_000_000_000,
        subject: "First".to_owned(),
        upstream: None,
        ahead: None,
        behind: None,
        worktree: None,
    };
    let commit = GitLogCommit {
        id: "0a1b".to_owned(),
        parents: vec!["2c3d".to_owned()],
        subject: "First".to_owned(),
        author: "Ada".to_owned(),
        time: 1_700_000_000_000,
        refs: vec![GitRef {
            name: "main".to_owned(),
            kind: GitRefKind::Head,
        }],
        side: GitSide::Local,
    };

    assert_eq!(
        serde_json::to_value(branch).unwrap(),
        json!({
            "name": "origin/main",
            "kind": "remote",
            "tip": "0a1b",
            "time": 1_700_000_000_000_i64,
            "subject": "First",
            "upstream": null,
            "ahead": null,
            "behind": null,
            "worktree": null
        })
    );
    assert_eq!(
        serde_json::to_value(commit).unwrap(),
        json!({
            "id": "0a1b",
            "parents": ["2c3d"],
            "subject": "First",
            "author": "Ada",
            "time": 1_700_000_000_000_i64,
            "refs": [{ "name": "main", "kind": "head" }],
            "side": "local"
        })
    );
}

#[test]
fn reads_the_params_of_the_branch_and_history_methods() {
    let switch: GitSwitchParams =
        serde_json::from_value(json!({ "repo": "/r", "branch": "origin/a", "kind": "remote" }))
            .unwrap();
    let log: GitLogParams = serde_json::from_value(json!({ "repo": "/r" })).unwrap();
    let paged: GitLogParams =
        serde_json::from_value(json!({ "repo": "/r", "limit": 200, "skip": 50 })).unwrap();
    let diff: GitDiffParams =
        serde_json::from_value(json!({ "repo": "/r", "path": "a.txt", "commit": "0a1b" })).unwrap();
    let working_tree: GitDiffParams =
        serde_json::from_value(json!({ "repo": "/r", "path": "a.txt" })).unwrap();

    assert_eq!(switch.kind, GitSwitchKind::Remote);
    assert_eq!((log.limit, log.skip), (None, None));
    assert_eq!((paged.limit, paged.skip), (Some(200), Some(50)));
    assert_eq!(diff.commit.as_deref(), Some("0a1b"));
    assert_eq!(working_tree.commit, None);
    assert!(
        serde_json::from_value::<GitSwitchParams>(
            json!({ "repo": "/r", "branch": "a", "kind": "force" })
        )
        .is_err()
    );
    assert!(serde_json::from_value::<GitLogParams>(json!({ "repo": "/r", "limit": -1 })).is_err());
    assert!(serde_json::from_value::<GitLogParams>(json!({ "repo": "/r", "skip": 1.5 })).is_err());
}

#[test]
fn a_download_needs_both_expectations_present() {
    let first: FsDownloadParams = serde_json::from_value(json!({
        "path": "/a", "offset": 0, "length": 1, "expected_size": null, "expected_mtime": null
    }))
    .unwrap();
    assert_eq!(first.expected_size, None);

    let missing = serde_json::from_value::<FsDownloadParams>(json!({
        "path": "/a", "offset": 0, "length": 1, "expected_size": null
    }));
    assert!(missing.is_err());
    let fraction = serde_json::from_value::<FsDownloadParams>(json!({
        "path": "/a", "offset": 0.5, "length": 1, "expected_size": null, "expected_mtime": null
    }));
    assert!(fraction.is_err());
}

#[test]
fn reads_an_upload_destination_and_what_to_do_about_a_taken_name() {
    let into_directory: FsUploadBeginParams = serde_json::from_value(json!({
        "destination": { "kind": "dir", "path": "/a" },
        "name": "b.txt",
        "size": 3,
        "on_conflict": "rename"
    }))
    .unwrap();
    assert_eq!(
        into_directory.destination,
        FsUploadDestination::Dir {
            path: "/a".to_owned()
        }
    );
    assert_eq!(into_directory.on_conflict, Some(FsOnConflict::Rename));

    let into_uploads: FsUploadBeginParams = serde_json::from_value(json!({
        "destination": { "kind": "uploads" }, "name": "b.png", "size": 3
    }))
    .unwrap();
    assert_eq!(into_uploads.destination, FsUploadDestination::Uploads);
    assert_eq!(into_uploads.on_conflict, None);

    let unknown = serde_json::from_value::<FsUploadBeginParams>(json!({
        "destination": { "kind": "desktop" }, "name": "b.png", "size": 3
    }));
    assert!(unknown.is_err());
}
