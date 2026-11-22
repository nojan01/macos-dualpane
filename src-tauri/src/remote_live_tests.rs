//! Opt-in live tests; never use or execute a saved synchronization profile.
use super::*;

#[test]
#[ignore = "Requires DUALBEAM_TEST_PROFILE_DB and Keychain authorization; writes only a unique test directory"]
fn saved_smb_profile_roundtrip() {
    let db = std::env::var("DUALBEAM_TEST_PROFILE_DB").expect("explicit profile database required");
    let output = Command::new("/usr/bin/sqlite3")
        .args([
            "-readonly",
            &db,
            "select hex(value) from ItemTable where key='dualbeam:remote-profiles:v1'",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let hex = String::from_utf8(output.stdout).unwrap();
    let bytes: Vec<_> = hex
        .trim()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|v| u8::from_str_radix(std::str::from_utf8(v).unwrap(), 16).unwrap())
        .collect();
    let words: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|v| u16::from_le_bytes([v[0], v[1]]))
        .collect();
    let profiles: Vec<RemoteSpec> =
        serde_json::from_str(&String::from_utf16(&words).unwrap()).unwrap();
    let mut spec = profiles
        .into_iter()
        .find(|p| p.protocol == RemoteProtocol::Smb)
        .expect("saved SMB profile missing");
    let nonce = format!(
        "dualbeam-smb-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    spec.label = nonce.clone();
    let password = remote_password(&spec)
        .unwrap()
        .expect("SMB password missing from Keychain");
    let mount =
        PathBuf::from(mount_blocking(spec.clone(), password, false).expect("SMB mount failed"));
    let root = mount.join(&nonce);
    let context = rclone_transfer_context(&root).expect("SMB direct transfer context missing");
    assert!(
        !rclone_path_exists(&root).unwrap().unwrap(),
        "test prefix collision"
    );
    let local = std::env::temp_dir().join(&nonce);
    std::fs::create_dir(&local).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let src = local.join("source");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        for name in ["file.txt", "nested/Grüße mit Leerzeichen.txt", ".hidden"] {
            std::fs::write(src.join(name), b"original").unwrap();
        }
        let cancel = AtomicBool::new(false);
        let copy = |from: &Path, to: &Path, download, overwrite| {
            copy_rclone_storage(
                &context,
                download,
                from,
                to,
                overwrite,
                &cancel,
                &mut |_| {},
            )
            .unwrap()
        };
        copy(&src, &root, false, true);
        let dst = local.join("download");
        copy(&root, &dst, true, true);
        for name in ["file.txt", "nested/Grüße mit Leerzeichen.txt", ".hidden"] {
            assert_eq!(
                std::fs::read(src.join(name)).unwrap(),
                std::fs::read(dst.join(name)).unwrap()
            );
        }
        crate::verify_move_copy(&src, &root).unwrap();
        eprintln!("SMB: mount, nested upload/download and content verification PASS");
        std::fs::write(src.join("file.txt"), b"modified").unwrap();
        copy(&src.join("file.txt"), &root.join("file.txt"), false, false);
        copy(&root.join("file.txt"), &local.join("single"), true, true);
        assert_eq!(std::fs::read(local.join("single")).unwrap(), b"original");
        let plan = crate::sync_preview_inner(
            src.to_str().unwrap(),
            root.to_str().unwrap(),
            true,
            vec![],
            true,
            None,
        )
        .unwrap();
        assert!(plan
            .iter()
            .any(|e| e.rel == "file.txt" && e.action == "update"));
        copy(&src.join("file.txt"), &root.join("file.txt"), false, true);
        assert!(crate::sync_preview_inner(
            src.to_str().unwrap(),
            root.to_str().unwrap(),
            true,
            vec![],
            true,
            None
        )
        .unwrap()
        .is_empty());
        eprintln!("SMB: overwrite protection and checksum sync PASS");
        let extra = root.join("extra.txt");
        copy(&src.join("file.txt"), &extra, false, true);
        let plan = crate::sync_preview_inner(
            src.to_str().unwrap(),
            root.to_str().unwrap(),
            true,
            vec![],
            false,
            None,
        )
        .unwrap();
        assert!(plan
            .iter()
            .any(|e| e.rel == "extra.txt" && e.action == "delete"));
        assert!(!crate::sync_preview_inner(
            src.to_str().unwrap(),
            root.to_str().unwrap(),
            true,
            vec!["extra.txt".into()],
            false,
            None
        )
        .unwrap()
        .iter()
        .any(|e| e.rel == "extra.txt"));
        purge_remote_storage(
            &spec,
            &mount,
            std::slice::from_ref(&extra),
            &cancel,
            None,
            |_| {},
        )
        .unwrap();
        assert!(!rclone_path_exists(&extra).unwrap().unwrap());
        let aborted = copy_rclone_storage(
            &context,
            false,
            &src.join("file.txt"),
            &root.join("cancelled"),
            true,
            &AtomicBool::new(true),
            &mut |_| {},
        );
        assert_eq!(aborted.unwrap_err(), "JOB_CANCELLED");
        assert!(!rclone_path_exists(&root.join("cancelled"))
            .unwrap()
            .unwrap());
        eprintln!(
            "SMB: delete preview, exclusions, direct deletion and pre-transfer cancellation PASS"
        );
    }));
    assert_eq!(root.parent(), Some(mount.as_path()));
    let cleanup = purge_remote_storage(
        &spec,
        &mount,
        std::slice::from_ref(&root),
        &AtomicBool::new(false),
        None,
        |_| {},
    );
    let absent = rclone_path_exists(&root);
    let unmounted = unmount_owned(&mount);
    let local_cleanup = std::fs::remove_dir_all(&local);
    cleanup.expect("SMB test cleanup failed");
    assert!(!absent.unwrap().unwrap());
    unmounted.unwrap();
    local_cleanup.unwrap();
    eprintln!("SMB: test data removed and test mount released");
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
