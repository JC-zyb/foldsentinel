use super::*;
use std::sync::{Arc, Mutex};

fn control(dir: &Path) -> Control {
    Control::with_recovery_dir(dir.join("jobs"), |_| {})
}
fn round_trip(bytes: &[u8]) {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input");
    let output = dir.path().join("sealed");
    let restored = dir.path().join("restored");
    fs::write(&input, bytes).unwrap();
    let encrypted = encrypt_with_control(&input, &output, "test", &control(dir.path())).unwrap();
    assert_eq!(encrypted.input_bytes, bytes.len() as u64);
    decrypt_with_control(&output, &restored, "test", &control(dir.path())).unwrap();
    assert_eq!(fs::read(restored).unwrap(), bytes);
    assert_eq!(fs::read(input).unwrap(), bytes);
    assert!(
        recovery::interrupted_jobs(&dir.path().join("jobs"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn v2_chunk_boundaries_round_trip() {
    for size in [
        0,
        1,
        stream::CHUNK - 1,
        stream::CHUNK,
        stream::CHUNK + 1,
        stream::CHUNK * 2,
    ] {
        round_trip(&vec![0xA7; size]);
    }
}

#[test]
fn actual_v01_release_fixtures_remain_readable() {
    for (fixture, folder) in [
        (
            include_bytes!("../../tests/fixtures/v1-file.fse").as_slice(),
            false,
        ),
        (
            include_bytes!("../../tests/fixtures/v1-folder.fse").as_slice(),
            true,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("v1.fse");
        let output = dir.path().join("restored");
        fs::write(&input, fixture).unwrap();
        let report = decrypt_with_control(
            &input,
            &output,
            "compatibility-test-only",
            &control(dir.path()),
        )
        .unwrap();
        assert_eq!(report.directory, folder);
        let file = if folder {
            assert!(output.join("empty").is_dir());
            output.join("内容.txt")
        } else {
            output
        };
        assert_eq!(
            fs::read(file).unwrap(),
            b"FoldSentinel v0.1 compatibility\n"
        );
    }
}

#[test]
fn v2_folder_round_trip_and_progress() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir_all(source.join("空目录")).unwrap();
    fs::write(source.join("中文.txt"), vec![42; stream::CHUNK + 13]).unwrap();
    let long_name = "长".repeat(45) + ".txt";
    fs::write(source.join(&long_name), b"long path").unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let c = Control::with_recovery_dir(dir.path().join("jobs"), move |p| {
        capture.lock().unwrap().push(p)
    });
    let sealed = dir.path().join("sealed");
    encrypt_with_control(&source, &sealed, "test", &c).unwrap();
    let output = dir.path().join("restored");
    decrypt_with_control(&sealed, &output, "test", &control(dir.path())).unwrap();
    assert!(output.join("空目录").is_dir());
    assert_eq!(fs::read(output.join(long_name)).unwrap(), b"long path");
    assert_eq!(
        fs::read(output.join("中文.txt")).unwrap(),
        fs::read(source.join("中文.txt")).unwrap()
    );
    let events = events.lock().unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.phase, "completed");
    assert_eq!(last.completed, last.total);
    assert_eq!(last.total, (stream::CHUNK + 13 + 9) as u64);
    assert!(events.iter().any(|p| p.phase == "committing"));
}

#[test]
fn oversized_tar_extension_rejected_before_body_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut header = [0; HEADER_LEN];
    header[..8].copy_from_slice(MAGIC);
    header[8] = 2;
    header[9] = 1;
    header[33] = 20;
    header[42..46].copy_from_slice(&1u32.to_le_bytes());
    let mut tar_header = tar::Header::new_gnu();
    tar_header.set_path("././@LongLink").unwrap();
    tar_header.set_size(1024 * 1024 * 1024);
    tar_header.set_entry_type(tar::EntryType::GNULongName);
    tar_header.set_cksum();
    let sealed = dir.path().join("sealed");
    let mut writer = EncryptWriter::new(
        File::create(&sealed).unwrap(),
        "test",
        header,
        control(dir.path()),
    )
    .unwrap();
    writer.write_all(tar_header.as_bytes()).unwrap();
    writer.finish().unwrap();
    let output = dir.path().join("restored");
    let error = decrypt_with_control(&sealed, &output, "test", &control(dir.path())).unwrap_err();
    assert!(format!("{error:#}").contains("扩展名称超限"));
    assert!(!output.exists());
}

#[test]
fn tampering_reordering_truncation_and_append_never_commit() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let sealed = dir.path().join("sealed");
    let output = dir.path().join("output");
    fs::write(&source, vec![7; stream::CHUNK * 2 + 13]).unwrap();
    encrypt_with_control(&source, &sealed, "test", &control(dir.path())).unwrap();
    let original = fs::read(&sealed).unwrap();
    let frame = 4 + stream::CHUNK + 16;
    let mut changes = Vec::new();
    for offset in [9, 10, 26, 34, HEADER_LEN + 4, original.len() - 1] {
        let mut changed = original.clone();
        changed[offset] ^= 1;
        changes.push(changed);
    }
    let mut reordered = original.clone();
    reordered[HEADER_LEN..HEADER_LEN + frame]
        .copy_from_slice(&original[HEADER_LEN + frame..HEADER_LEN + frame * 2]);
    reordered[HEADER_LEN + frame..HEADER_LEN + frame * 2]
        .copy_from_slice(&original[HEADER_LEN..HEADER_LEN + frame]);
    changes.push(reordered);
    changes.push(original[..HEADER_LEN + frame * 2].to_vec());
    changes.push(original[..original.len() - 1].to_vec());
    let mut appended = original.clone();
    appended.push(0);
    changes.push(appended);
    for bytes in changes {
        fs::write(&sealed, bytes).unwrap();
        assert!(decrypt_with_control(&sealed, &output, "test", &control(dir.path())).is_err());
        assert!(!output.exists());
        assert!(
            recovery::interrupted_jobs(&dir.path().join("jobs"))
                .unwrap()
                .is_empty()
        );
    }
    fs::write(&sealed, original).unwrap();
    assert!(decrypt_with_control(&sealed, &output, "wrong", &control(dir.path())).is_err());
    assert!(!output.exists());
}

#[test]
fn cancellation_before_commit_cleans_payload_and_journal() {
    for phase in ["encrypting", "syncing"] {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input");
        fs::write(&input, vec![7; stream::CHUNK + 1]).unwrap();
        let output = dir.path().join("output");
        let token = Arc::new(Mutex::new(None::<Control>));
        let capture = token.clone();
        let c = Control::with_recovery_dir(dir.path().join("jobs"), move |p| {
            if p.phase == phase {
                assert!(capture.lock().unwrap().as_ref().unwrap().cancel());
            }
        });
        *token.lock().unwrap() = Some(c.clone());
        let result = encrypt_with_control(&input, &output, "test", &c);
        token.lock().unwrap().take();
        assert!(result.unwrap_err().to_string().contains("取消"));
        assert!(!output.exists());
        assert!(input.exists());
        assert!(
            recovery::interrupted_jobs(&dir.path().join("jobs"))
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn interrupted_job_cleanup_cannot_remove_final_output_or_active_job() {
    let dir = tempfile::tempdir().unwrap();
    let registry = dir.path().join("jobs");
    let input = dir.path().join("input");
    let output = dir.path().join("output");
    fs::write(&input, b"source").unwrap();
    let job = recovery::Job::create(&input, &output, true, &registry).unwrap();
    let id = job
        .payload()
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    fs::write(job.payload(), b"partial plaintext").unwrap();
    assert!(recovery::interrupted_jobs(&registry).unwrap().is_empty());
    assert!(recovery::cleanup_interrupted(&registry, &id).is_err());
    drop(job);
    fs::write(&output, b"keep final").unwrap();
    assert_eq!(recovery::interrupted_jobs(&registry).unwrap().len(), 1);
    recovery::cleanup_interrupted(&registry, &id).unwrap();
    assert_eq!(fs::read(output).unwrap(), b"keep final");
    assert!(input.exists());
    assert!(recovery::interrupted_jobs(&registry).unwrap().is_empty());
    assert!(recovery::cleanup_interrupted(&registry, "../outside").is_err());
}

#[test]
fn stream_write_failure_propagates_disk_full_error() {
    struct Full {
        remaining: usize,
    }
    impl Write for Full {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Err(std::io::Error::from_raw_os_error(112));
            }
            let count = bytes.len().min(self.remaining);
            self.remaining -= count;
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut header = [0; HEADER_LEN];
    header[..8].copy_from_slice(MAGIC);
    header[8] = 2;
    header[33] = 20;
    assert!(
        EncryptWriter::new(Full { remaining: 0 }, "test", header, control(dir.path())).is_err()
    );
    let mut writer = EncryptWriter::new(
        Full {
            remaining: HEADER_LEN + stream::CHUNK + 20 + 10,
        },
        "test",
        header,
        control(dir.path()),
    )
    .unwrap();
    let error = writer.write_all(&vec![7; stream::CHUNK * 2]).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(112));
}

#[test]
fn directory_end_requires_authenticated_final_frame() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), vec![1; stream::CHUNK - 1536]).unwrap();
    let sealed = dir.path().join("sealed");
    encrypt_with_control(&source, &sealed, "test", &control(dir.path())).unwrap();
    let mut bytes = fs::read(&sealed).unwrap();
    bytes.truncate(HEADER_LEN + 4 + stream::CHUNK + 16);
    fs::write(&sealed, bytes).unwrap();
    let output = dir.path().join("restored");
    assert!(decrypt_with_control(&sealed, &output, "test", &control(dir.path())).is_err());
    assert!(!output.exists());
}

#[test]
fn changed_source_and_commit_race_never_overwrite_output() {
    for phase in ["verifying", "committing"] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file"), b"unchanged").unwrap();
        let output = dir.path().join("sealed");
        let capture_source = source.clone();
        let capture_output = output.clone();
        let c = Control::with_recovery_dir(dir.path().join("jobs"), move |p| {
            if p.phase == phase {
                if phase == "verifying" {
                    fs::write(capture_source.join("new"), b"added").unwrap();
                } else {
                    fs::write(&capture_output, b"raced output").unwrap();
                }
            }
        });
        assert!(encrypt_with_control(&source, &output, "test", &c).is_err());
        if phase == "committing" {
            assert_eq!(fs::read(output).unwrap(), b"raced output");
            assert!(!c.cancel());
        } else {
            assert!(!output.exists());
        }
        assert_eq!(fs::read(source.join("file")).unwrap(), b"unchanged");
        assert!(
            recovery::interrupted_jobs(&dir.path().join("jobs"))
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn cancelled_restore_removes_temporary_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input");
    let sealed = dir.path().join("sealed");
    let output = dir.path().join("restored");
    fs::write(&input, vec![3; stream::CHUNK + 13]).unwrap();
    encrypt_with_control(&input, &sealed, "test", &control(dir.path())).unwrap();
    let token = Arc::new(Mutex::new(None::<Control>));
    let capture = token.clone();
    let c = Control::with_recovery_dir(dir.path().join("jobs"), move |p| {
        if p.phase == "syncing" {
            assert!(capture.lock().unwrap().as_ref().unwrap().cancel());
        }
    });
    *token.lock().unwrap() = Some(c.clone());
    let result = decrypt_with_control(&sealed, &output, "test", &c);
    token.lock().unwrap().take();
    assert!(result.unwrap_err().to_string().contains("取消"));
    assert!(!output.exists());
    assert!(
        recovery::interrupted_jobs(&dir.path().join("jobs"))
            .unwrap()
            .is_empty()
    );
    assert!(!fs::read_dir(dir.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".foldsentinel-job-")
    }));
}
