//! Regression coverage for transfer, sync, archive and rename safety.
use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "dualbeam-safety-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn roots(&self) -> (PathBuf, PathBuf) {
        let src = self.0.join("src");
        let dst = self.0.join("dst");
        std::fs::create_dir(&src).unwrap();
        std::fs::create_dir(&dst).unwrap();
        (src, dst)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cancelled_move_keeps_source() {
    let f = Fixture::new();
    let src = f.0.join("file");
    std::fs::write(&src, b"original").unwrap();
    assert_eq!(
        remove_source_after_move(&src, CopyOutcome::Copied, &AtomicBool::new(true)).unwrap_err(),
        "JOB_CANCELLED"
    );
    assert_eq!(std::fs::read(src).unwrap(), b"original");
}

#[test]
fn move_verification_rejects_missing_or_different_content() {
    let f = Fixture::new();
    let (src, dst) = f.roots();
    std::fs::write(src.join("file"), b"1234").unwrap();
    assert!(verify_move_copy(&src, &dst).is_err());
    std::fs::write(dst.join("file"), b"5678").unwrap();
    assert!(verify_move_copy(&src, &dst).is_err());
    std::fs::write(dst.join("file"), b"1234").unwrap();
    assert!(verify_move_copy(&src, &dst).is_ok());
    assert!(src.join("file").exists());
}

#[test]
fn sync_filters_children_and_creates_only_shallow_directories() {
    let f = Fixture::new();
    let (src, dst) = f.roots();
    std::fs::create_dir(src.join("project")).unwrap();
    std::fs::write(src.join("project/keep.txt"), b"ok").unwrap();
    std::fs::write(src.join("project/secret.env"), b"secret").unwrap();
    let rules = vec!["*.env".into()];
    let local = sync_preview_inner(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        false,
        rules.clone(),
        false,
        None,
    )
    .unwrap();
    let direct =
        direct_object_storage_sync_preview(&src, &dst, false, &rules, None, false).unwrap();
    for plan in [local, direct] {
        assert!(plan.iter().any(|e| e.rel == "project" && e.is_dir));
        assert!(plan.iter().any(|e| e.rel == "project/keep.txt"));
        assert!(!plan.iter().any(|e| e.rel.ends_with(".env")));
    }
    let item: JobItem = serde_json::from_value(serde_json::json!({
        "src": "source/project", "dst": "target/project", "overwrite": true, "directoryOnly": true
    }))
    .unwrap();
    assert!(item.directory_only);
}

#[test]
fn sync_never_deletes_a_parent_with_excluded_descendants() {
    let f = Fixture::new();
    let (src, dst) = f.roots();
    std::fs::create_dir_all(dst.join("old/nested")).unwrap();
    std::fs::write(dst.join("old/nested/secret.env"), b"secret").unwrap();
    std::fs::write(dst.join("old/stale.txt"), b"old").unwrap();
    let rules = vec!["*.env".into()];
    let local = sync_preview_inner(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        true,
        rules.clone(),
        false,
        None,
    )
    .unwrap();
    let direct = direct_object_storage_sync_preview(&src, &dst, true, &rules, None, false).unwrap();
    for plan in [local, direct] {
        let deletes: Vec<_> = plan.iter().filter(|e| e.action == "delete").collect();
        assert_eq!(deletes.len(), 1);
        assert_eq!(deletes[0].rel, "old/stale.txt");
    }
}

#[test]
fn direct_checksum_option_detects_equal_size_equal_time_changes() {
    let f = Fixture::new();
    let (src, dst) = f.roots();
    std::fs::write(src.join("file"), b"1234").unwrap();
    std::fs::write(dst.join("file"), b"5678").unwrap();
    assert!(
        direct_object_storage_sync_preview(&src, &dst, false, &[], None, false)
            .unwrap()
            .is_empty()
    );
    let plan = direct_object_storage_sync_preview(&src, &dst, false, &[], None, true).unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].action, "update");
}

#[test]
fn nfs_metadata_protects_only_valid_companions_of_retained_owners() {
    let f = Fixture::new();
    let (src, dst) = f.roots();
    for name in [
        "file",
        ".hidden",
        "Grüße",
        "ordinary",
        "truncated",
        "linked",
    ] {
        std::fs::write(src.join(name), b"data").unwrap();
        std::fs::write(dst.join(name), b"data").unwrap();
    }
    std::fs::create_dir(src.join("folder")).unwrap();
    std::fs::create_dir(dst.join("folder")).unwrap();
    for name in ["file", ".hidden", "Grüße", "folder"] {
        std::fs::write(dst.join(format!("._{name}")), nfs_metadata::test_fixture()).unwrap();
    }
    std::fs::write(dst.join("._ordinary"), b"a real user file").unwrap();
    std::fs::write(dst.join("._truncated"), &nfs_metadata::test_fixture()[..30]).unwrap();
    symlink("._file", dst.join("._linked")).unwrap();
    let mut local = Vec::new();
    preview_walk_dst(&src, &dst, &dst, &[], &mut local).unwrap();
    let direct = direct_object_storage_sync_preview(&src, &dst, true, &[], None, false).unwrap();
    for mut plan in [local, direct] {
        protect_appledouble_sync_metadata(&dst, &mut plan).unwrap();
        let mut deletes: Vec<_> = plan
            .iter()
            .filter(|e| e.action == "delete")
            .map(|e| e.rel.as_str())
            .collect();
        deletes.sort();
        assert_eq!(deletes, ["._linked", "._ordinary", "._truncated"]);
    }
}

#[test]
fn nfs_metadata_does_not_preserve_orphans_or_deleted_owners() {
    let f = Fixture::new();
    let (src, dst) = f.roots();
    std::fs::write(dst.join("obsolete"), b"remove").unwrap();
    std::fs::create_dir(dst.join("old")).unwrap();
    std::fs::write(dst.join("old/child"), b"remove").unwrap();
    for name in ["orphan", "obsolete", "old", "old/._child"] {
        let path = if name.contains('/') {
            dst.join(name)
        } else {
            dst.join(format!("._{name}"))
        };
        std::fs::write(path, nfs_metadata::test_fixture()).unwrap();
    }
    let mut plan = Vec::new();
    preview_walk_dst(&src, &dst, &dst, &[], &mut plan).unwrap();
    let before: Vec<_> = plan.iter().map(|e| e.rel.clone()).collect();
    protect_appledouble_sync_metadata(&dst, &mut plan).unwrap();
    assert_eq!(
        plan.iter().map(|e| e.rel.clone()).collect::<Vec<_>>(),
        before
    );
    assert!(plan.iter().any(|e| e.rel == "old" && e.is_dir));
}

#[test]
fn nfs_metadata_of_excluded_children_does_not_block_other_deletes() {
    let f = Fixture::new();
    let (src, dst) = f.roots();
    std::fs::create_dir(dst.join("old")).unwrap();
    std::fs::write(dst.join("old/keep.env"), b"retained").unwrap();
    std::fs::write(dst.join("old/stale.txt"), b"delete").unwrap();
    std::fs::write(dst.join("old/._keep.env"), nfs_metadata::test_fixture()).unwrap();
    std::fs::write(dst.join("._old"), nfs_metadata::test_fixture()).unwrap();
    let rules = vec!["keep.env".into()];
    let mut local = Vec::new();
    preview_walk_dst(&src, &dst, &dst, &rules, &mut local).unwrap();
    let direct = direct_object_storage_sync_preview(&src, &dst, true, &rules, None, false).unwrap();
    for mut plan in [local, direct] {
        protect_appledouble_sync_metadata(&dst, &mut plan).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].rel, "old/stale.txt");
    }
}

#[test]
fn nfs_metadata_policy_does_not_hide_local_or_source_sidecars() {
    let f = Fixture::new();
    let (src, dst) = f.roots();
    std::fs::write(src.join("file"), b"same").unwrap();
    std::fs::write(dst.join("file"), b"same").unwrap();
    std::fs::write(dst.join("._file"), nfs_metadata::test_fixture()).unwrap();
    assert_ne!(statfs_fstype(&dst).as_deref(), Some("nfs"));
    let plan = sync_preview_inner(
        src.to_str().unwrap(),
        dst.to_str().unwrap(),
        true,
        vec![],
        false,
        None,
    )
    .unwrap();
    assert!(plan
        .iter()
        .any(|e| e.rel == "._file" && e.action == "delete"));
    let plan = sync_preview_inner(
        dst.to_str().unwrap(),
        src.to_str().unwrap(),
        false,
        vec![],
        false,
        None,
    )
    .unwrap();
    assert!(plan.iter().any(|e| e.rel == "._file" && e.action == "copy"));
}

#[test]
#[ignore = "Opt-in live NFS check: DUALBEAM_NFS_TEST_ROOT must name an already mounted NFS directory"]
fn nfs_metadata_live_sync_preserves_generated_companions() {
    let mount =
        PathBuf::from(std::env::var("DUALBEAM_NFS_TEST_ROOT").expect("NFS test root required"));
    assert_eq!(statfs_fstype(&mount).as_deref(), Some("nfs"));
    let f = Fixture::new();
    let src = f.0.join("source");
    std::fs::create_dir(&src).unwrap();
    let name = format!(
        "dualbeam-nfs-metadata-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let dst = mount.join(&name);
    std::fs::create_dir(&dst).unwrap();
    let result = std::panic::catch_unwind(|| {
        std::fs::create_dir(src.join("nested")).unwrap();
        std::fs::create_dir(dst.join("nested")).unwrap();
        for name in ["file", ".hidden", "nested/Grüße.txt"] {
            std::fs::write(src.join(name), b"original").unwrap();
            copy_file_retry(
                &src.join(name),
                &dst.join(name),
                &AtomicBool::new(false),
                false,
                None,
            )
            .unwrap();
        }
        let mut raw = Vec::new();
        preview_walk_dst(&src, &dst, &dst, &[], &mut raw).unwrap();
        assert!(
            !raw.is_empty(),
            "server did not produce sidecars; live check inconclusive"
        );
        for entry in &raw {
            assert!(nfs_metadata::is_macos_appledouble(&dst.join(&entry.rel)).unwrap());
        }
        let preview = sync_preview_inner(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            true,
            vec![],
            true,
            None,
        )
        .unwrap();
        assert!(
            preview.is_empty(),
            "unchanged NFS sync must be empty without an ignore filter"
        );
        eprintln!(
            "NFS: {} genuine metadata companions preserved; unchanged sync empty",
            raw.len()
        );
        std::fs::write(dst.join("user-file"), b"owner").unwrap();
        std::fs::write(src.join("user-file"), b"owner").unwrap();
        std::fs::write(dst.join("._user-file"), b"ordinary data").unwrap();
        let preview = sync_preview_inner(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            true,
            vec![],
            true,
            None,
        )
        .unwrap();
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0].rel, "._user-file");
        assert_eq!(preview[0].action, "delete");
        eprintln!("NFS: ordinary ._ user file still detected as extra");
    });
    nfs_delete::remove_tree(&dst, &AtomicBool::new(false), |_| {}).unwrap();
    let companion = mount.join(format!("._{name}"));
    if companion.exists() && nfs_metadata::is_macos_appledouble(&companion).unwrap() {
        std::fs::remove_file(companion).unwrap();
    }
    assert!(!dst.exists());
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}

#[test]
fn zip_roundtrip_preserves_links_empty_directories_and_executables() {
    let f = Fixture::new();
    let src = f.0.join("app");
    std::fs::create_dir_all(src.join("empty")).unwrap();
    std::fs::write(src.join("binary"), b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(src.join("binary"), std::fs::Permissions::from_mode(0o755)).unwrap();
    symlink("binary", src.join("link")).unwrap();
    symlink("missing", src.join("dangling")).unwrap();
    let archive = f.0.join("app.zip");
    let out = f.0.join("out");
    zip_create_inner(
        vec![src.to_string_lossy().into_owned()],
        archive.to_string_lossy().into_owned(),
    )
    .unwrap();
    zip_extract_inner(
        archive.to_string_lossy().into_owned(),
        out.to_string_lossy().into_owned(),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_link(out.join("app/link")).unwrap(),
        Path::new("binary")
    );
    assert_eq!(
        std::fs::read_link(out.join("app/dangling")).unwrap(),
        Path::new("missing")
    );
    assert!(out.join("app/empty").is_dir());
    assert_eq!(
        std::fs::metadata(out.join("app/binary"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}

#[test]
fn zip_links_cannot_redirect_extraction_outside_destination() {
    let f = Fixture::new();
    let archive = f.0.join("bad.zip");
    let outside = f.0.join("outside");
    std::fs::create_dir(&outside).unwrap();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.add_symlink("pivot", outside.to_str().unwrap(), options)
        .unwrap();
    zip.start_file("pivot/escaped", options).unwrap();
    zip.write_all(b"bad").unwrap();
    zip.finish().unwrap();
    let out = f.0.join("out");
    assert!(zip_extract_inner(
        archive.to_string_lossy().into_owned(),
        out.to_string_lossy().into_owned()
    )
    .is_err());
    assert!(!outside.join("escaped").exists());
    assert!(!out.exists());
}

#[test]
fn case_rename_preserves_content_and_does_not_confuse_hardlinks() {
    let f = Fixture::new();
    let a = f.0.join("file.txt");
    let b = f.0.join("FILE.txt");
    std::fs::write(&a, b"original").unwrap();
    rename_path(
        a.to_string_lossy().into_owned(),
        b.to_string_lossy().into_owned(),
    )
    .unwrap();
    assert_eq!(std::fs::read(&b).unwrap(), b"original");
    assert!(std::fs::read_dir(&f.0)
        .unwrap()
        .any(|e| e.unwrap().file_name() == "FILE.txt"));
    let link = f.0.join("other.txt");
    std::fs::hard_link(&b, &link).unwrap();
    assert!(!same_directory_entry(&b, &link).unwrap());
    assert!(rename_path(
        b.to_string_lossy().into_owned(),
        link.to_string_lossy().into_owned()
    )
    .is_err());
}
