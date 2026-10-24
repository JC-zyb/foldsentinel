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
    let before = fs::read_dir(dir.path()).unwrap().count();
    let verified = verify_with_control(&output, "test", &control(dir.path())).unwrap();
    assert_eq!(verified.output_bytes, bytes.len() as u64);
    let inspection = inspect_with_control(&output, "test", &control(dir.path())).unwrap();
    assert_eq!(inspection.preview.format_version, 2);
    assert_eq!(inspection.preview.file_count, 1);
    assert_eq!(inspection.preview.content_bytes, bytes.len() as u64);
    assert_eq!(inspection.preview.entries[0].size, bytes.len() as u64);
    assert!(inspection.preview.entries[0].path.is_none());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), before);
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
        let verified =
            verify_with_control(&input, "compatibility-test-only", &control(dir.path())).unwrap();
        assert_eq!(verified.directory, folder);
        let inspected =
            inspect_with_control(&input, "compatibility-test-only", &control(dir.path())).unwrap();
        assert_eq!(inspected.preview.format_version, 1);
        assert_eq!(
            inspected.preview.content_bytes,
            b"FoldSentinel v0.1 compatibility\n".len() as u64
        );
        assert_eq!(inspected.preview.file_count, 1);
        if folder {
            assert!(
                inspected
                    .preview
                    .entries
                    .iter()
                    .any(|entry| entry.path.as_deref() == Some("empty") && entry.directory)
            );
            assert!(
                inspected
                    .preview
                    .entries
                    .iter()
                    .any(|entry| entry.path.as_deref() == Some("内容.txt") && !entry.directory)
            );
        } else {
            assert_eq!(inspected.preview.entries.len(), 1);
            assert!(inspected.preview.entries[0].path.is_none());
        }
        assert!(!dir.path().join("jobs").exists());
        let error = verify_with_control(&input, "wrong", &control(dir.path())).unwrap_err();
        assert_eq!(error_code(&error), ErrorCode::Authentication);
        assert!(inspect_with_control(&input, "wrong", &control(dir.path())).is_err());
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
fn inspection_is_bounded_and_still_authenticates_the_end() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("many.fse");
    for malformed in [false, true] {
        let count = MAX_PREVIEW_ENTRIES + 3;
        let mut archive = tar::Builder::new(Vec::new());
        for index in 0..count {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o600);
            header.set_entry_type(if malformed && index == count - 1 {
                tar::EntryType::Symlink
            } else {
                tar::EntryType::Regular
            });
            header.set_cksum();
            archive
                .append_data(&mut header, format!("条目-{index}.txt"), &[][..])
                .unwrap();
        }
        let bytes = archive.into_inner().unwrap();
        let mut header = [0; HEADER_LEN];
        header[..8].copy_from_slice(MAGIC);
        header[8] = 2;
        header[9] = 1;
        header[33] = 20;
        header[42..46].copy_from_slice(&(count as u32).to_le_bytes());
        let mut writer = EncryptWriter::new(
            File::create(&input).unwrap(),
            "test",
            header,
            control(dir.path()),
        )
        .unwrap();
        writer.write_all(&bytes).unwrap();
        writer.finish().unwrap();
        let inspection = inspect_with_control(&input, "test", &control(dir.path()));
        if malformed {
            assert!(inspection.is_err(), "展示上限之后的非法条目仍须拒绝");
            continue;
        }
        let inspection = inspection.unwrap();
        assert_eq!(inspection.preview.total_entries, count);
        assert_eq!(inspection.preview.entries.len(), MAX_PREVIEW_ENTRIES);
        assert_eq!(inspection.preview.content_bytes, 0);
        assert!(inspection.preview.truncated);
        let mut damaged = fs::read(&input).unwrap();
        *damaged.last_mut().unwrap() ^= 1;
        fs::write(&input, &damaged).unwrap();
        assert!(inspect_with_control(&input, "test", &control(dir.path())).is_err());
        fs::write(&input, &damaged[..damaged.len() - 1]).unwrap();
        assert!(inspect_with_control(&input, "test", &control(dir.path())).is_err());
    }
    let cancelled = control(dir.path());
    assert!(cancelled.cancel());
    assert_eq!(
        error_code(&inspect_with_control(&input, "test", &cancelled).unwrap_err()),
        ErrorCode::Cancelled
    );
    assert!(!dir.path().join("jobs").exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn inspection_handles_empty_folders_and_cancellation_without_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("empty");
    let sealed = dir.path().join("empty.fse");
    fs::create_dir(&source).unwrap();
    encrypt_with_control(&source, &sealed, "test", &control(dir.path())).unwrap();
    let before = fs::read_dir(dir.path()).unwrap().count();
    let inspection = inspect_with_control(&sealed, "test", &control(dir.path())).unwrap();
    assert!(inspection.report.directory);
    assert_eq!(inspection.preview.total_entries, 0);
    assert_eq!(inspection.preview.content_bytes, 0);
    assert!(inspection.preview.entries.is_empty());
    let token = Arc::new(Mutex::new(None::<Control>));
    let captured = token.clone();
    let registry = dir.path().join("preview-jobs");
    let c = Control::with_recovery_dir(registry.clone(), move |p| {
        assert!(p.current.is_empty());
        if p.phase == "checking" {
            captured.lock().unwrap().as_ref().unwrap().cancel();
        }
    });
    *token.lock().unwrap() = Some(c.clone());
    let result = inspect_with_control(&sealed, "test", &c);
    token.lock().unwrap().take();
    assert_eq!(error_code(&result.unwrap_err()), ErrorCode::Cancelled);
    assert!(!registry.exists());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), before);
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
    assert!(
        verify_with_control(&sealed, "test", &control(dir.path()))
            .unwrap()
            .directory
    );
    let output = dir.path().join("restored");
    let preview_events = Arc::new(Mutex::new(Vec::new()));
    let preview_capture = preview_events.clone();
    let preview_control = Control::with_recovery_dir(dir.path().join("preview-jobs"), move |p| {
        preview_capture.lock().unwrap().push(p);
    });
    let inspection = inspect_with_control(&sealed, "test", &preview_control).unwrap();
    assert!(
        preview_events
            .lock()
            .unwrap()
            .iter()
            .all(|p| p.current.is_empty())
    );
    assert!(!dir.path().join("preview-jobs").exists());
    assert_eq!(inspection.preview.format_version, 2);
    assert_eq!(inspection.preview.total_entries, 3);
    assert_eq!(inspection.preview.file_count, 2);
    assert_eq!(inspection.preview.directory_count, 1);
    assert_eq!(
        inspection.preview.content_bytes,
        (stream::CHUNK + 13 + 9) as u64
    );
    assert!(
        inspection
            .preview
            .entries
            .iter()
            .any(|entry| entry.path.as_deref() == Some("空目录") && entry.directory)
    );
    assert!(
        inspection
            .preview
            .entries
            .iter()
            .any(|entry| entry.path.as_deref() == Some(&long_name) && entry.size == 9)
    );
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
fn malformed_archive_metadata_is_rejected_without_reading_attacker_sized_bodies() {
    struct HeaderOnly(std::io::Cursor<Vec<u8>>);
    impl Read for HeaderOnly {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            assert!(self.0.position() < 512, "解析器不得读取被拒绝条目的正文");
            self.0.read(buf)
        }
    }
    let dir = tempfile::tempdir().unwrap();
    for kind in [
        tar::EntryType::GNULongName,
        tar::EntryType::GNULongLink,
        tar::EntryType::XHeader,
        tar::EntryType::XGlobalHeader,
        tar::EntryType::GNUSparse,
        tar::EntryType::Regular,
    ] {
        for size in [4098, u32::MAX as u64, u64::MAX] {
            let mut header = tar::Header::new_gnu();
            header.set_path("entry").unwrap();
            header.set_size(size);
            header.set_entry_type(kind);
            header.set_cksum();
            let mut reader = HeaderOnly(std::io::Cursor::new(header.as_bytes().to_vec()));
            assert!(
                process_directory(&mut reader, None, Some((0, 1)), &control(dir.path()), None)
                    .is_err()
            );
        }
    }
    // GNU 名称即使长度在界限内，也必须包含结尾 NUL，且不能指向上级路径。
    for name in [b"no-final-nul".as_slice(), b"../escape\0".as_slice()] {
        let mut archive = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(name.len() as u64);
        header.set_entry_type(tar::EntryType::GNULongName);
        header.set_cksum();
        archive
            .append_data(&mut header, "././@LongLink", name)
            .unwrap();
        let mut file = tar::Header::new_gnu();
        file.set_size(0);
        file.set_mode(0o600);
        file.set_cksum();
        archive
            .append_data(&mut file, "placeholder", &[][..])
            .unwrap();
        let bytes = archive.into_inner().unwrap();
        assert!(
            process_directory(
                &mut &bytes[..],
                None,
                Some((0, 1)),
                &control(dir.path()),
                None
            )
            .is_err()
        );
    }
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
        assert!(verify_with_control(&sealed, "test", &control(dir.path())).is_err());
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
    for phase in ["deriving", "encrypting", "verifying", "syncing"] {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input");
        fs::create_dir(&input).unwrap();
        fs::write(input.join("file"), vec![7; stream::CHUNK + 1]).unwrap();
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
fn cancellation_after_commit_gate_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("source");
    let output = dir.path().join("package");
    fs::create_dir(&input).unwrap();
    fs::write(input.join("file"), b"keep source").unwrap();
    let token = Arc::new(Mutex::new(None::<Control>));
    let captured = token.clone();
    let c = Control::with_recovery_dir(dir.path().join("jobs"), move |p| {
        if p.phase == "committing" {
            assert!(!captured.lock().unwrap().as_ref().unwrap().cancel());
        }
    });
    *token.lock().unwrap() = Some(c.clone());
    encrypt_with_control(&input, &output, "test", &c).unwrap();
    token.lock().unwrap().take();
    verify_with_control(&output, "test", &control(dir.path())).unwrap();
    assert_eq!(fs::read(input.join("file")).unwrap(), b"keep source");
}

#[cfg(windows)]
#[test]
fn extended_windows_paths_round_trip_and_readonly_output_reports_permission() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mut deep = fs::canonicalize(dir.path()).unwrap();
    for n in 0..7 {
        deep.push(format!("segment-{n}-{}", "a".repeat(40)));
    }
    fs::create_dir_all(&deep).unwrap();
    assert!(deep.as_os_str().len() > 300);
    let input = deep.join("中文.bin");
    let sealed = deep.join("package.fse");
    let restored = deep.join("restored.bin");
    fs::write(&input, b"long path").unwrap();
    encrypt_with_control(&input, &sealed, "test", &control(dir.path())).unwrap();
    verify_with_control(&sealed, "test", &control(dir.path())).unwrap();
    decrypt_with_control(&sealed, &restored, "test", &control(dir.path())).unwrap();
    assert_eq!(fs::read(restored).unwrap(), b"long path");
    // 私有测试文件设只读，不更改用户 ACL 或系统设置。
    let denied = deep.join("readonly-target");
    fs::write(&denied, b"keep").unwrap();
    let original_permissions = fs::metadata(&denied).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&denied, permissions).unwrap();
    let error = OpenOptions::new().write(true).open(&denied).unwrap_err();
    assert_eq!(error_code(&error.into()), ErrorCode::PermissionDenied);
    let error = decrypt_with_control(&sealed, &denied, "test", &control(dir.path())).unwrap_err();
    assert_eq!(error_code(&error), ErrorCode::OutputExists);
    fs::set_permissions(&denied, original_permissions).unwrap();
    // 真实独占句柄模拟源文件被其他进程占用，失败后不得创建输出。
    let locked = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&input)
        .unwrap();
    let failed_output = deep.join("locked.fse");
    assert!(encrypt_with_control(&input, &failed_output, "test", &control(dir.path())).is_err());
    assert!(!failed_output.exists());
    drop(locked);
    assert_eq!(fs::read(denied).unwrap(), b"keep");
}

#[test]
fn verification_cancels_without_creating_journals_or_outputs() {
    let dir = tempfile::tempdir().unwrap();
    for fixture in [
        include_bytes!("../../tests/fixtures/v1-file.fse").as_slice(),
        include_bytes!("../../tests/fixtures/v1-folder.fse").as_slice(),
    ] {
        let input = dir.path().join("v1.fse");
        fs::write(&input, fixture).unwrap();
        let token = Arc::new(Mutex::new(None::<Control>));
        let captured = token.clone();
        let c = Control::with_recovery_dir(dir.path().join("jobs"), move |p| {
            if p.phase == "deriving" {
                captured.lock().unwrap().as_ref().unwrap().cancel();
            }
        });
        *token.lock().unwrap() = Some(c.clone());
        let error = verify_with_control(&input, "compatibility-test-only", &c).unwrap_err();
        token.lock().unwrap().take();
        assert_eq!(error_code(&error), ErrorCode::Cancelled);
        assert!(!dir.path().join("jobs").exists());
    }
    let input = dir.path().join("file");
    let sealed = dir.path().join("v2.fse");
    fs::write(&input, vec![13; stream::CHUNK * 2]).unwrap();
    encrypt_with_control(&input, &sealed, "test", &control(dir.path())).unwrap();
    let registry = dir.path().join("verify-jobs");
    let token = Arc::new(Mutex::new(None::<Control>));
    let captured = token.clone();
    let c = Control::with_recovery_dir(registry.clone(), move |p| {
        if p.phase == "checking" {
            captured.lock().unwrap().as_ref().unwrap().cancel();
        }
    });
    *token.lock().unwrap() = Some(c.clone());
    let error = verify_with_control(&sealed, "test", &c).unwrap_err();
    token.lock().unwrap().take();
    assert_eq!(error_code(&error), ErrorCode::Cancelled);
    assert!(!registry.exists());
}

#[test]
fn verification_rejects_archive_file_parent_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    for paths in [["parent", "parent/child"], ["parent/child", "parent"]] {
        let mut archive = tar::Builder::new(Vec::new());
        for path in paths {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o600);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            archive.append_data(&mut header, path, &[][..]).unwrap();
        }
        let source = archive.into_inner().unwrap();
        assert!(
            process_directory(
                &mut &source[..],
                None,
                Some((0, 2)),
                &control(dir.path()),
                None
            )
            .is_err()
        );
    }
}

#[test]
fn preflight_and_error_categories_are_explicit() {
    let locked =
        anyhow::Error::from(std::io::Error::from_raw_os_error(32)).context(ErrorCode::WriteFailed);
    assert_eq!(error_code(&locked), ErrorCode::WriteFailed);
    assert_eq!(
        error_code(&locked.context(ErrorCode::Recovery)),
        ErrorCode::Recovery
    );
    assert!(space::require_space(100, 100).is_ok());
    let error = space::require_space(99, 100).unwrap_err();
    assert_eq!(error_code(&error), ErrorCode::InsufficientSpace);
    for (error, code) in [
        (
            std::io::Error::from_raw_os_error(112),
            ErrorCode::InsufficientSpace,
        ),
        (
            std::io::Error::from_raw_os_error(5),
            ErrorCode::PermissionDenied,
        ),
        (
            std::io::Error::from_raw_os_error(183),
            ErrorCode::OutputExists,
        ),
        (
            std::io::Error::other(ErrorCode::Authentication),
            ErrorCode::Authentication,
        ),
    ] {
        assert_eq!(error_code(&error.into()), code);
    }
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("unknown");
    fs::write(&input, b"FSEAL999unsupported").unwrap();
    assert_eq!(
        error_code(&verify_with_control(&input, "test", &control(dir.path())).unwrap_err()),
        ErrorCode::UnsupportedFormat
    );
    assert_eq!(
        error_code(
            &verify_with_control(&dir.path().join("missing"), "test", &control(dir.path()))
                .unwrap_err()
        ),
        ErrorCode::ReadFailed
    );
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
    let journal = registry.join(format!("{id}.json"));
    let original_record = fs::read(&journal).unwrap();
    let mut altered: serde_json::Value = serde_json::from_slice(&original_record).unwrap();
    altered["output"] = serde_json::json!(dir.path().join(id.to_ascii_uppercase()));
    fs::write(&journal, serde_json::to_vec(&altered).unwrap()).unwrap();
    assert!(recovery::cleanup_interrupted(&registry, &id).is_err());
    assert!(dir.path().join(&id).exists());
    fs::write(&journal, original_record).unwrap();
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
