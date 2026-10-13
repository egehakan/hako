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
    ];

    assert_eq!(
        serde_json::to_value(codes).unwrap(),
        json!([
            "agent_offline",
            "timeout",
            "bad_request",
            "not_found",
            "not_supported",
            "failed"
        ])
    );
}

#[test]
fn knows_the_workspace_methods_only() {
    assert_eq!(Method::from_name("app.info"), Some(Method::AppInfo));
    assert_eq!(
        Method::from_name("workspace.list"),
        Some(Method::WorkspaceList)
    );
    assert_eq!(Method::from_name("tab.create"), Some(Method::TabCreate));
    assert_eq!(Method::from_name("tab.activate"), Some(Method::TabActivate));
    assert_eq!(Method::from_name("tab.close"), Some(Method::TabClose));
    assert_eq!(Method::from_name("term.subscribe"), None);
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
