use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::thread;

use super::*;

fn start_request(text: &str) -> StartRequest {
    match parse_request(text).unwrap() {
        PreviewRequest::Start(request) => request,
        other => panic!("expected a start request, got {other:?}"),
    }
}

fn web() -> StartRequest {
    start_request(
        r#"{ "action": "start", "id": "web", "name": "Web", "directory": "/tmp/web",
             "command": "npm run dev", "port": 5173, "env": { "PORT": "5173" } }"#,
    )
}

#[test]
fn checks_preview_ids() {
    assert_eq!(check_id("my-app2"), Ok(()));
    for id in ["", "-a", "a-", "My", "a_b", "api", &"a".repeat(41)] {
        assert!(check_id(id).is_err(), "{id}");
    }
    assert!(check_id("my--app").unwrap_err().contains("--"));
}

#[test]
fn reads_a_start_request_with_defaults_and_extra_ports() {
    let request = start_request(
        r#"{ "action": "start", "id": "web", "directory": "/w", "command": "pnpm dev",
             "port": 8086, "label": "web", "ports": [8087, { "port": 9000, "label": "minio" }] }"#,
    );
    assert_eq!(request.url_path, "/");
    assert_eq!(request.ready_path, "/");
    assert_eq!(request.repo, None);
    assert_eq!(
        preview_ports(8086, Some("web"), "Web", &request.ports).unwrap(),
        [
            PreviewPort {
                port: 8086,
                label: "web".to_owned()
            },
            PreviewPort {
                port: 8087,
                label: "8087".to_owned()
            },
            PreviewPort {
                port: 9000,
                label: "minio".to_owned()
            },
        ]
    );
    let (id, message) = parse_request(r#"{ "action": "start", "id": "web" }"#).unwrap_err();
    assert_eq!(id.as_deref(), Some("web"));
    assert!(message.contains("directory"), "{message}");
}

#[test]
fn exports_the_environment_for_the_command_only() {
    let env = BTreeMap::from([
        ("A".to_owned(), "it's".to_owned()),
        ("B".to_owned(), "2".to_owned()),
    ]);
    assert_eq!(
        command_line("/w", "npm run dev", &BTreeMap::new(), ShellSyntax::Posix),
        "cd '/w' && npm run dev"
    );
    assert_eq!(
        command_line("/my dir", "npm run dev", &env, ShellSyntax::Posix),
        r"cd '/my dir' && (export A='it'\''s' B='2'; npm run dev)"
    );
    assert_eq!(
        command_line("/w", "npm run dev", &env, ShellSyntax::Fish),
        r"cd '/w' && begin; set -lx A 'it\'s'; set -lx B '2'; npm run dev; end"
    );
}

/// Answers every connection with `answer` once it has read the request's first line.
fn server(answer: &'static [u8]) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let _ = stream.write_all(answer);
        }
    });
    port
}

#[test]
fn a_server_answers_with_any_status_and_a_closed_port_does_not() {
    let wait = Duration::from_secs(2);
    assert!(answers(
        server(b"HTTP/1.1 404 Not Found\r\n\r\n"),
        "/",
        wait
    ));
    assert!(!answers(server(b"SSH-2.0\r\n"), "/", wait));
    let closed = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    assert!(!answers(port, "/", wait));
}

#[test]
fn takes_only_finished_request_files_and_keeps_the_state_whole() {
    let directory = tempfile::tempdir().unwrap();
    let requests = directory.path().join(REQUESTS_DIR);
    fs::create_dir_all(&requests).unwrap();
    fs::write(
        requests.join("a.json"),
        r#"{ "action": "stop", "id": "web" }"#,
    )
    .unwrap();
    fs::write(requests.join(".b.json.tmp"), "{").unwrap();
    fs::write(requests.join("c.json"), "{").unwrap();
    let taken = take_requests(&requests);
    assert_eq!(taken.len(), 2);
    assert!(matches!(
        taken
            .iter()
            .find(|file| file.file == "a.json")
            .unwrap()
            .request,
        Ok(PreviewRequest::Stop(_))
    ));
    assert!(
        taken
            .iter()
            .any(|file| file.file == "c.json" && file.request.is_err())
    );
    assert!(requests.join(".b.json.tmp").exists());
    assert!(!requests.join("a.json").exists());

    let now = Instant::now();
    let mut registry = Registry::default();
    registry.start(web(), 1, now);
    let path = directory.path().join(STATE_FILE);
    write_state(&path, &registry.state(2)).unwrap();
    let restored = Registry::restore(read_state(&path), now);
    let preview = &restored.list()[0];
    assert_eq!(preview.status, PreviewStatus::Stopped);
    assert_eq!(preview.message.as_deref(), Some("Hako was restarted"));
    assert_eq!(restored.get("web").unwrap().env["PORT"], "5173");
}

/// Runs the checks due at `now` with the outcome `answered`.
fn check_all(registry: &mut Registry, answered: bool, now: Instant) -> Vec<Checked> {
    registry
        .due_checks(now)
        .iter()
        .map(|check| registry.checked(check, answered, now))
        .collect()
}

fn status(registry: &Registry) -> PreviewStatus {
    registry.list()[0].status
}

#[test]
fn a_started_preview_runs_once_it_answers_and_stops_when_it_no_longer_does() {
    let now = Instant::now();
    let mut registry = Registry::default();
    registry.start(web(), 1, now);
    assert_eq!(registry.launching(), [("web".to_owned(), None)]);
    assert!(registry.open_ports().contains_key("web"));
    registry.set_tab("web", "t1".to_owned(), "s1".to_owned());
    registry.launched("web", now);
    assert_eq!(
        check_all(&mut registry, false, now + READY_POLL_INTERVAL),
        [Checked::Unchanged]
    );
    assert_eq!(status(&registry), PreviewStatus::Starting);
    let now = now + 2 * READY_POLL_INTERVAL;
    assert_eq!(check_all(&mut registry, true, now), [Checked::Ready]);
    assert_eq!(status(&registry), PreviewStatus::Running);

    // Answers missing for less than the grace period change nothing.
    let later = now + HEALTH_INTERVAL;
    assert_eq!(check_all(&mut registry, false, later), [Checked::Unchanged]);
    let later = now + HEALTH_GRACE + HEALTH_INTERVAL;
    assert_eq!(check_all(&mut registry, false, later), [Checked::Changed]);
    assert_eq!(status(&registry), PreviewStatus::Stopped);
    assert!(!registry.open_ports().contains_key("web"));

    let later = later + HEALTH_INTERVAL;
    assert_eq!(check_all(&mut registry, true, later), [Checked::Changed]);
    assert_eq!(status(&registry), PreviewStatus::Running);
}

#[test]
fn a_command_that_ends_before_answering_fails_and_one_that_is_too_slow_too() {
    let now = Instant::now();
    let mut registry = Registry::default();
    registry.start(web(), 1, now);
    registry.launched("web", now);
    assert!(registry.command_ended("web", 127, now));
    assert_eq!(status(&registry), PreviewStatus::Failed);
    assert!(registry.list()[0].message.as_ref().unwrap().contains("127"));

    registry.restart("web", 5, now).unwrap();
    registry.launched("web", now);
    let late = now + READY_TIMEOUT + READY_POLL_INTERVAL;
    assert_eq!(check_all(&mut registry, false, late), [Checked::Changed]);
    assert_eq!(status(&registry), PreviewStatus::Failed);
}

#[test]
fn stopping_interrupts_the_tab_and_waits_for_the_server_to_go() {
    let now = Instant::now();
    let mut registry = Registry::default();
    registry.start(web(), 1, now);
    registry.set_tab("web", "t1".to_owned(), "s1".to_owned());
    registry.launched("web", now);
    check_all(&mut registry, true, now + READY_POLL_INTERVAL);

    let outcome = registry.stop("web", now).unwrap();
    assert_eq!(
        outcome.work,
        [TabWork::Interrupt {
            tab_id: "t1".to_owned()
        }]
    );
    assert_eq!(status(&registry), PreviewStatus::Running);
    assert!(registry.command_ended("web", 130, now));
    assert_eq!(status(&registry), PreviewStatus::Stopped);

    // A stopped preview is not brought back by answers, but a restart runs it in the same tab.
    assert!(check_all(&mut registry, true, now + HEALTH_INTERVAL).is_empty());
    let outcome = registry.restart("web", 5, now).unwrap();
    assert!(outcome.work.is_empty());
    assert_eq!(
        registry.launching(),
        [("web".to_owned(), Some("t1".to_owned()))]
    );

    let outcome = registry.remove("web").unwrap();
    assert_eq!(
        outcome.work,
        [TabWork::Close {
            tab_id: "t1".to_owned()
        }]
    );
    assert!(registry.list().is_empty());
}

#[test]
fn a_repeated_start_leaves_a_running_server_and_replaces_a_changed_one() {
    let now = Instant::now();
    let mut registry = Registry::default();
    registry.start(web(), 1, now);
    registry.set_tab("web", "t1".to_owned(), "s1".to_owned());
    registry.launched("web", now);
    check_all(&mut registry, true, now + READY_POLL_INTERVAL);

    let outcome = registry.start(web(), 2, now);
    assert!(outcome.work.is_empty());
    assert_eq!(status(&registry), PreviewStatus::Running);
    assert_eq!(registry.list()[0].started, 1);

    let mut changed = web();
    changed.command = "npm start".to_owned();
    let outcome = registry.start(changed, 3, now);
    assert_eq!(
        outcome.work,
        [TabWork::Interrupt {
            tab_id: "t1".to_owned()
        }]
    );
    assert_eq!(status(&registry), PreviewStatus::Starting);
    assert_eq!(
        registry.launching(),
        [("web".to_owned(), Some("t1".to_owned()))]
    );
}

#[test]
fn a_request_with_a_bad_id_fails_with_the_reason() {
    let now = Instant::now();
    let mut registry = Registry::default();
    let mut request = web();
    request.id = "my--app".to_owned();
    registry.start(request, 1, now);
    let preview = &registry.list()[0];
    assert_eq!(preview.status, PreviewStatus::Failed);
    assert!(preview.message.as_ref().unwrap().contains("--"));
    assert!(registry.open_ports().is_empty());
}
