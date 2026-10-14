use std::fmt::Debug;
use std::os::unix::fs::symlink;

use tempfile::TempDir;

use super::*;

const TODAY: &str = "2026-10-13";

/// A data directory with its transfers, and a directory to upload into.
struct Fixture {
    data_dir: TempDir,
    transfers: Transfers,
    folder: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let data_dir = tempfile::tempdir().unwrap();
        let transfers = Transfers::new(data_dir.path().to_owned());
        Self {
            data_dir,
            transfers,
            folder: tempfile::tempdir().unwrap(),
        }
    }

    fn folder(&self) -> String {
        self.folder.path().to_str().unwrap().to_owned()
    }

    fn begin(
        &self,
        destination: FsUploadDestination,
        name: &str,
        size: u64,
        on_conflict: Option<FsOnConflict>,
    ) -> Result<String, RpcError> {
        self.transfers
            .begin(
                FsUploadBeginParams {
                    destination,
                    name: name.to_owned(),
                    size,
                    on_conflict,
                },
                Instant::now(),
                TODAY,
            )
            .map(|begun| begun.upload_id)
    }

    fn begin_in_folder(
        &self,
        name: &str,
        size: u64,
        on_conflict: Option<FsOnConflict>,
    ) -> Result<String, RpcError> {
        let destination = FsUploadDestination::Dir {
            path: self.folder(),
        };
        self.begin(destination, name, size, on_conflict)
    }

    fn chunk(&self, id: &str, offset: u64, bytes: &[u8]) -> Result<u64, RpcError> {
        self.chunk_at(id, offset, bytes, Instant::now())
    }

    fn chunk_at(&self, id: &str, offset: u64, bytes: &[u8], now: Instant) -> Result<u64, RpcError> {
        let params = FsUploadChunkParams {
            upload_id: id.to_owned(),
            offset,
            data: BASE64.encode(bytes),
        };
        self.transfers
            .chunk(params, now)
            .map(|chunk| chunk.received)
    }

    fn end(&self, id: &str) -> Result<FsUploadEnd, RpcError> {
        self.end_at(id, Instant::now())
    }

    fn end_at(&self, id: &str, now: Instant) -> Result<FsUploadEnd, RpcError> {
        self.transfers.end(id_params(id), now)
    }

    /// Uploads `bytes` in one piece.
    fn upload(
        &self,
        destination: FsUploadDestination,
        name: &str,
        bytes: &[u8],
        on_conflict: Option<FsOnConflict>,
    ) -> Result<FsUploadEnd, RpcError> {
        let id = self.begin(destination, name, bytes.len() as u64, on_conflict)?;
        if !bytes.is_empty() {
            self.chunk(&id, 0, bytes)?;
        }
        self.end(&id)
    }

    fn upload_to_folder(
        &self,
        name: &str,
        bytes: &[u8],
        on_conflict: Option<FsOnConflict>,
    ) -> Result<FsUploadEnd, RpcError> {
        let destination = FsUploadDestination::Dir {
            path: self.folder(),
        };
        self.upload(destination, name, bytes, on_conflict)
    }

    fn temporaries(&self, directory: &Path) -> Vec<PathBuf> {
        fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(TEMPORARY_PREFIX)
            })
            .collect()
    }

    fn record(&self) -> PathBuf {
        self.data_dir.path().join(UNFINISHED_RECORD)
    }
}

fn id_params(id: &str) -> FsUploadIdParams {
    FsUploadIdParams {
        upload_id: id.to_owned(),
    }
}

fn code<T: Debug>(result: Result<T, RpcError>) -> ErrorCode {
    result.unwrap_err().code
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn download_params(
    path: &str,
    offset: u64,
    length: u64,
    expected: Option<(u64, i64)>,
) -> FsDownloadParams {
    FsDownloadParams {
        path: path.to_owned(),
        offset,
        length,
        expected_size: expected.map(|(size, _)| size),
        expected_mtime: expected.map(|(_, mtime)| mtime),
    }
}

fn decoded(download: &FsDownload) -> Vec<u8> {
    BASE64.decode(&download.data).unwrap()
}

#[test]
fn reports_the_limits() {
    assert_eq!(
        info(),
        FsTransferInfo {
            max_bytes: 209_715_200,
            chunk_bytes: 1_048_576
        }
    );
}

#[test]
fn downloads_a_file_in_pieces_on_the_condition_of_the_first() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("a.bin");
    let contents: Vec<u8> = (0..=255).cycle().take(2500).collect();
    fs::write(&file, &contents).unwrap();
    let path = file.to_str().unwrap();

    let first = download(&download_params(path, 0, 1000, None)).unwrap();
    assert_eq!(first.size, 2500);
    let expected = Some((first.size, first.mtime));
    let second = download(&download_params(path, 1000, 1000, expected)).unwrap();
    let last = download(&download_params(path, 2000, 1000, expected)).unwrap();
    // Any piece may be asked for again.
    let again = download(&download_params(path, 1000, 1000, expected)).unwrap();

    let whole: Vec<u8> = [&first, &second, &last]
        .into_iter()
        .flat_map(decoded)
        .collect();
    assert_eq!(whole, contents);
    assert_eq!(decoded(&again), decoded(&second));
    let at_end = download(&download_params(path, 2500, 1000, expected)).unwrap();
    assert_eq!(at_end.data, "");
}

#[test]
fn downloads_an_empty_file_as_one_empty_piece() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("empty");
    fs::write(&file, "").unwrap();

    let piece = download(&download_params(
        file.to_str().unwrap(),
        0,
        CHUNK_BYTES,
        None,
    ))
    .unwrap();

    assert_eq!((piece.data.as_str(), piece.size), ("", 0));
}

#[test]
fn answers_conflict_when_the_file_changed_since_the_first_piece() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("a.txt");
    fs::write(&file, "hello").unwrap();
    let path = file.to_str().unwrap();
    let first = download(&download_params(path, 0, 2, None)).unwrap();

    let other_size = download(&download_params(path, 2, 2, Some((6, first.mtime))));
    let other_mtime = download(&download_params(path, 2, 2, Some((5, first.mtime + 1))));

    assert_eq!(code(other_size), ErrorCode::Conflict);
    assert_eq!(code(other_mtime), ErrorCode::Conflict);
}

#[test]
fn refuses_a_download_it_cannot_serve() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("a.txt");
    fs::write(&file, "hello").unwrap();
    let path = file.to_str().unwrap();
    let directory = folder.path().to_str().unwrap();

    let only_one_expectation = FsDownloadParams {
        expected_size: Some(5),
        ..download_params(path, 0, 1, None)
    };
    assert_eq!(code(download(&only_one_expectation)), ErrorCode::BadRequest);
    assert_eq!(
        code(download(&download_params(path, 0, 0, None))),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(download(&download_params(path, 0, CHUNK_BYTES + 1, None))),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(download(&download_params(path, 6, 1, None))),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(download(&download_params("a.txt", 0, 1, None))),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(download(&download_params(
            &format!("{directory}/missing"),
            0,
            1,
            None
        ))),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(download(&download_params(directory, 0, 1, None))),
        ErrorCode::IsADirectory
    );

    let large = folder.path().join("large.bin");
    File::create(&large)
        .unwrap()
        .set_len(MAX_BYTES + 1)
        .unwrap();
    assert_eq!(
        code(download(&download_params(
            large.to_str().unwrap(),
            0,
            1,
            None
        ))),
        ErrorCode::TooLarge
    );

    fs::set_permissions(&file, Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        code(download(&download_params(path, 0, 1, None))),
        ErrorCode::PermissionDenied
    );
}

#[test]
fn uploads_into_a_directory_through_a_hidden_temporary_file() {
    let fixture = Fixture::new();
    let id = fixture.begin_in_folder("notes.txt", 11, None).unwrap();

    let temporaries = fixture.temporaries(fixture.folder.path());
    assert_eq!(
        temporaries,
        vec![fixture.folder.path().join(format!(".hako-upload-{id}"))]
    );
    assert_eq!(mode(&temporaries[0]), 0o600);
    assert!(!fixture.folder.path().join("notes.txt").exists());

    assert_eq!(fixture.chunk(&id, 0, b"hello ").unwrap(), 6);
    assert!(!fixture.folder.path().join("notes.txt").exists());
    assert_eq!(fixture.chunk(&id, 6, b"world").unwrap(), 11);
    let placed = fixture.end(&id).unwrap();

    let file = fixture.folder.path().join("notes.txt");
    assert_eq!(fs::read(&file).unwrap(), b"hello world");
    assert_eq!(placed.path, file.to_str().unwrap());
    assert_eq!(placed.name, "notes.txt");
    assert_eq!(placed.size, 11);
    assert_eq!(placed.mtime, mtime(&fs::metadata(&file).unwrap()));
    assert_eq!(mode(&file), new_file_mode());
    assert_eq!(new_file_mode() & !0o644, 0);
    assert!(fixture.temporaries(fixture.folder.path()).is_empty());
}

#[test]
fn takes_a_repeated_piece_and_refuses_one_out_of_order() {
    let fixture = Fixture::new();
    let id = fixture.begin_in_folder("a.txt", 6, None).unwrap();
    fixture.chunk(&id, 0, b"abc").unwrap();

    assert_eq!(fixture.chunk(&id, 0, b"abc").unwrap(), 3);
    assert_eq!(fixture.chunk(&id, 1, b"bc").unwrap(), 3);
    assert_eq!(code(fixture.chunk(&id, 4, b"e")), ErrorCode::BadRequest);
    assert_eq!(code(fixture.chunk(&id, 2, b"cd")), ErrorCode::BadRequest);
    assert_eq!(code(fixture.chunk(&id, 3, b"defg")), ErrorCode::BadRequest);
    assert_eq!(code(fixture.chunk(&id, 3, b"")), ErrorCode::BadRequest);
    let not_base64 = FsUploadChunkParams {
        upload_id: id.clone(),
        offset: 3,
        data: "ZGVm\n".to_owned(),
    };
    assert_eq!(
        code(fixture.transfers.chunk(not_base64, Instant::now())),
        ErrorCode::BadRequest
    );
    let unpadded = FsUploadChunkParams {
        upload_id: id.clone(),
        offset: 3,
        data: "ZGU".to_owned(),
    };
    assert_eq!(
        code(fixture.transfers.chunk(unpadded, Instant::now())),
        ErrorCode::BadRequest
    );
    let oversized = FsUploadChunkParams {
        upload_id: id.clone(),
        offset: 3,
        data: BASE64.encode(vec![0; CHUNK_BYTES as usize + 1]),
    };
    assert_eq!(
        code(fixture.transfers.chunk(oversized, Instant::now())),
        ErrorCode::BadRequest
    );

    // Not every byte has arrived: the upload stays open.
    assert_eq!(code(fixture.end(&id)), ErrorCode::BadRequest);
    assert_eq!(fixture.chunk(&id, 3, b"def").unwrap(), 6);
    fixture.end(&id).unwrap();
    assert_eq!(
        fs::read(fixture.folder.path().join("a.txt")).unwrap(),
        b"abcdef"
    );
}

#[test]
fn answers_a_repeated_end_from_memory_for_ten_minutes() {
    let fixture = Fixture::new();
    let id = fixture.begin_in_folder("a.txt", 1, None).unwrap();
    fixture.chunk(&id, 0, b"a").unwrap();
    let ended_at = Instant::now();
    let first = fixture.end_at(&id, ended_at).unwrap();

    let again = fixture.end_at(&id, ended_at + RESULT_KEPT_FOR - Duration::from_secs(1));

    assert_eq!(again.unwrap(), first);
    assert_eq!(
        code(fixture.end_at(&id, ended_at + RESULT_KEPT_FOR)),
        ErrorCode::NotFound
    );
    assert_eq!(code(fixture.chunk(&id, 0, b"a")), ErrorCode::NotFound);
}

#[test]
fn uploads_an_empty_file() {
    let fixture = Fixture::new();

    let placed = fixture.upload_to_folder("empty", b"", None).unwrap();

    assert_eq!(placed.size, 0);
    assert_eq!(fs::read(fixture.folder.path().join("empty")).unwrap(), b"");
}

#[test]
fn refuses_an_upload_it_cannot_begin() {
    let fixture = Fixture::new();
    let folder = fixture.folder();
    fs::write(fixture.folder.path().join("file"), "x").unwrap();
    let into = |path: String| FsUploadDestination::Dir { path };

    for name in ["", ".", "..", "a/b", "a\0b"] {
        assert_eq!(
            code(fixture.begin_in_folder(name, 1, None)),
            ErrorCode::BadRequest,
            "{name:?}"
        );
    }
    assert_eq!(
        code(fixture.begin_in_folder(&"a".repeat(256), 1, None)),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(fixture.begin_in_folder("a", MAX_BYTES + 1, None)),
        ErrorCode::TooLarge
    );
    fixture.begin_in_folder("a", MAX_BYTES, None).unwrap();
    assert_eq!(
        code(fixture.begin(into(format!("{folder}/")), "a", 1, None)),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(fixture.begin(into(format!("{folder}/missing")), "a", 1, None)),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(fixture.begin(into(format!("{folder}/file")), "a", 1, None)),
        ErrorCode::NotADirectory
    );
}

#[test]
fn refuses_a_directory_that_may_not_be_written_to_before_a_byte_is_sent() {
    let fixture = Fixture::new();
    fs::set_permissions(fixture.folder.path(), Permissions::from_mode(0o500)).unwrap();

    let refused = fixture.begin_in_folder("a.txt", 1, None);

    fs::set_permissions(fixture.folder.path(), Permissions::from_mode(0o700)).unwrap();
    let error = refused.unwrap_err();
    assert_eq!(error.code, ErrorCode::PermissionDenied);
    assert!(
        error.message.contains(&fixture.folder()),
        "{}",
        error.message
    );
}

#[test]
fn fails_on_a_taken_name_at_begin_and_at_end() {
    let fixture = Fixture::new();
    let file = fixture.folder.path().join("a.txt");
    fs::write(&file, "old").unwrap();
    assert_eq!(
        code(fixture.begin_in_folder("a.txt", 3, Some(FsOnConflict::Fail))),
        ErrorCode::Conflict
    );
    assert_eq!(
        code(fixture.begin_in_folder("a.txt", 3, None)),
        ErrorCode::Conflict
    );
    assert!(fixture.temporaries(fixture.folder.path()).is_empty());

    let late = fixture.folder.path().join("b.txt");
    let id = fixture.begin_in_folder("b.txt", 3, None).unwrap();
    fixture.chunk(&id, 0, b"new").unwrap();
    fs::write(&late, "late").unwrap();

    assert_eq!(code(fixture.end(&id)), ErrorCode::Conflict);
    assert_eq!(fs::read(&late).unwrap(), b"late");
    assert!(fixture.temporaries(fixture.folder.path()).is_empty());
    assert_eq!(code(fixture.end(&id)), ErrorCode::NotFound);
}

#[test]
fn replaces_a_file_keeping_its_permissions() {
    let fixture = Fixture::new();
    let file = fixture.folder.path().join("a.sh");
    fs::write(&file, "old").unwrap();
    fs::set_permissions(&file, Permissions::from_mode(0o750)).unwrap();

    let placed = fixture
        .upload_to_folder("a.sh", b"new", Some(FsOnConflict::Replace))
        .unwrap();

    assert_eq!(placed.name, "a.sh");
    assert_eq!(fs::read(&file).unwrap(), b"new");
    assert_eq!(mode(&file), 0o750);
}

#[test]
fn replaces_a_link_itself_and_never_a_directory() {
    let fixture = Fixture::new();
    let target = fixture.folder.path().join("target.txt");
    fs::write(&target, "target").unwrap();
    let link = fixture.folder.path().join("link.txt");
    symlink(&target, &link).unwrap();
    fs::create_dir(fixture.folder.path().join("dir")).unwrap();

    fixture
        .upload_to_folder("link.txt", b"new", Some(FsOnConflict::Replace))
        .unwrap();

    assert!(fs::symlink_metadata(&link).unwrap().is_file());
    assert_eq!(fs::read(&link).unwrap(), b"new");
    assert_eq!(fs::read(&target).unwrap(), b"target");
    assert_eq!(
        code(fixture.begin_in_folder("dir", 1, Some(FsOnConflict::Replace))),
        ErrorCode::IsADirectory
    );

    let id = fixture
        .begin_in_folder("later", 1, Some(FsOnConflict::Replace))
        .unwrap();
    fixture.chunk(&id, 0, b"x").unwrap();
    fs::create_dir(fixture.folder.path().join("later")).unwrap();
    assert_eq!(code(fixture.end(&id)), ErrorCode::IsADirectory);
    assert!(fixture.temporaries(fixture.folder.path()).is_empty());
}

#[test]
fn keeps_both_by_numbering_the_new_file() {
    let fixture = Fixture::new();
    fs::write(fixture.folder.path().join("notes.txt"), "1").unwrap();
    fs::write(fixture.folder.path().join("notes (2).txt"), "2").unwrap();

    let placed = fixture
        .upload_to_folder("notes.txt", b"3", Some(FsOnConflict::Rename))
        .unwrap();
    let free = fixture
        .upload_to_folder("free.txt", b"4", Some(FsOnConflict::Rename))
        .unwrap();

    assert_eq!(placed.name, "notes (3).txt");
    assert_eq!(placed.path, format!("{}/notes (3).txt", fixture.folder()));
    assert_eq!(
        fs::read(fixture.folder.path().join("notes.txt")).unwrap(),
        b"1"
    );
    assert_eq!(free.name, "free.txt");
}

#[test]
fn uploads_into_the_uploads_area_under_a_clean_name() {
    let fixture = Fixture::new();
    let area = fixture.data_dir.path().join("uploads");

    let first = fixture
        .upload(
            FsUploadDestination::Uploads,
            "Screen Shot (1).PNG",
            b"png",
            Some(FsOnConflict::Replace),
        )
        .unwrap();
    let second = fixture
        .upload(
            FsUploadDestination::Uploads,
            "Screen Shot (1).PNG",
            b"png",
            None,
        )
        .unwrap();

    let day = area.join(TODAY);
    assert_eq!(first.name, "Screen-Shot-1.png");
    assert_eq!(first.path, day.join("Screen-Shot-1.png").to_str().unwrap());
    assert_eq!(second.name, "Screen-Shot-1-2.png");
    assert_eq!(fs::read(&second.path).unwrap(), b"png");
    assert_eq!(mode(Path::new(&first.path)), 0o600);
    assert_eq!(mode(&area), 0o700);
    assert_eq!(mode(&day), 0o700);
    let below_area = first.path.strip_prefix(area.to_str().unwrap()).unwrap();
    assert!(
        below_area
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte)),
        "{below_area}"
    );
}

#[test]
fn cancels_an_upload_and_removes_its_temporary_file() {
    let fixture = Fixture::new();
    let id = fixture.begin_in_folder("a.txt", 6, None).unwrap();
    fixture.chunk(&id, 0, b"abc").unwrap();

    fixture
        .transfers
        .cancel(id_params(&id), Instant::now())
        .unwrap();

    assert!(fixture.temporaries(fixture.folder.path()).is_empty());
    assert_eq!(code(fixture.chunk(&id, 3, b"def")), ErrorCode::NotFound);
    assert!(!fixture.folder.path().join("a.txt").exists());
    fixture
        .transfers
        .cancel(id_params("unknown"), Instant::now())
        .unwrap();
    assert_eq!(
        code(
            fixture
                .transfers
                .cancel(id_params("not an id"), Instant::now())
        ),
        ErrorCode::BadRequest
    );
}

#[test]
fn gives_up_on_an_upload_that_no_request_names_for_fifteen_minutes() {
    let fixture = Fixture::new();
    let id = fixture.begin_in_folder("a.txt", 2, None).unwrap();
    let named_at = Instant::now();
    fixture.chunk_at(&id, 0, b"a", named_at).unwrap();

    fixture
        .transfers
        .expire(named_at + UNFINISHED_KEPT_FOR - Duration::from_secs(1));
    assert_eq!(fixture.temporaries(fixture.folder.path()).len(), 1);
    fixture.transfers.expire(named_at + UNFINISHED_KEPT_FOR);

    assert!(fixture.temporaries(fixture.folder.path()).is_empty());
    assert_eq!(
        code(fixture.chunk_at(&id, 1, b"b", named_at + UNFINISHED_KEPT_FOR)),
        ErrorCode::NotFound
    );
    assert!(!fixture.record().exists());
}

#[test]
fn removes_the_leftovers_of_a_run_that_could_not_clean_up() {
    let fixture = Fixture::new();
    let id = fixture.begin_in_folder("a.txt", 2, None).unwrap();
    fixture.chunk(&id, 0, b"a").unwrap();
    assert!(fixture.record().exists());
    let unrelated = fixture.folder.path().join("keep.txt");
    fs::write(&unrelated, "keep").unwrap();
    let mut listed: Vec<PathBuf> =
        serde_json::from_slice(&fs::read(fixture.record()).unwrap()).unwrap();
    listed.push(unrelated.clone());
    fs::write(fixture.record(), serde_json::to_vec(&listed).unwrap()).unwrap();

    // The next run of the app.
    let next = Transfers::new(fixture.data_dir.path().to_owned());
    next.start(SystemTime::now(), TODAY);

    assert!(fixture.temporaries(fixture.folder.path()).is_empty());
    assert!(unrelated.exists());
    assert!(!fixture.record().exists());
}

#[test]
fn abandons_every_unfinished_upload_at_quitting() {
    let fixture = Fixture::new();
    fixture.begin_in_folder("a.txt", 2, None).unwrap();
    fixture
        .begin(FsUploadDestination::Uploads, "b.png", 2, None)
        .unwrap();

    fixture.transfers.abandon_all();

    assert!(fixture.temporaries(fixture.folder.path()).is_empty());
    let day = fixture.data_dir.path().join("uploads").join(TODAY);
    assert!(fixture.temporaries(&day).is_empty());
    assert!(!fixture.record().exists());
}
