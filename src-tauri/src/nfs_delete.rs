//! Bounded parallel unlink requests for real NFS mounts. NFS has no recursive
//! directory-delete operation; parents are removed only after their children.
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::time::Duration;

const WORKERS: usize = 8;

fn check_cancelled(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::SeqCst) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Löschen abgebrochen",
        ))
    } else {
        Ok(())
    }
}

fn remove_entry(path: &Path, directory: bool, cancel: &AtomicBool) -> io::Result<()> {
    for attempt in 0..=4 {
        check_cancelled(cancel)?;
        let result = if directory {
            std::fs::remove_dir(path)
        } else {
            std::fs::remove_file(path)
        };
        match result {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error)
                if attempt < 4
                    && matches!(
                        error.raw_os_error(),
                        Some(libc::EBUSY) | Some(libc::ENOTEMPTY)
                    ) =>
            {
                // Check cancellation throughout the retry delay, not just between files.
                for _ in 0..(3 << attempt) {
                    check_cancelled(cancel)?;
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("{}: {error}", path.display()),
                ))
            }
        }
    }
    unreachable!()
}

fn remove_files(
    paths: &[PathBuf],
    cancel: &AtomicBool,
    operation: impl Fn(&Path) -> io::Result<()> + Sync,
    on_removed: &mut impl FnMut(&Path),
) -> io::Result<()> {
    check_cancelled(cancel)?;
    if paths.len() <= 1 {
        for path in paths {
            operation(path)?;
            on_removed(path);
        }
        return Ok(());
    }
    let next = AtomicUsize::new(0);
    let stopped = AtomicBool::new(false);
    let (sender, receiver) = sync_channel(WORKERS);
    std::thread::scope(|scope| {
        for _ in 0..WORKERS.min(paths.len()) {
            let sender = sender.clone();
            let operation = &operation;
            let next = &next;
            let stopped = &stopped;
            scope.spawn(move || {
                while !stopped.load(Ordering::SeqCst) && !cancel.load(Ordering::SeqCst) {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(path) = paths.get(index) else { break };
                    let result = operation(path);
                    if result.is_err() {
                        stopped.store(true, Ordering::SeqCst);
                    }
                    if sender.send((index, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        let mut failure = None;
        // Emit progress on the calling thread only. All started requests finish
        // before returning, including after an error or cancellation.
        for (index, result) in receiver {
            match result {
                Ok(()) => on_removed(&paths[index]),
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                }
            }
        }
        if let Some(error) = failure {
            Err(error)
        } else {
            check_cancelled(cancel)
        }
    })
}

pub(crate) fn remove_tree(
    root: &Path,
    cancel: &AtomicBool,
    mut on_removed: impl FnMut(&Path),
) -> io::Result<()> {
    let mut pending = vec![(root.to_path_buf(), false)];
    while let Some((path, children_removed)) = pending.pop() {
        check_cancelled(cancel)?;
        if children_removed {
            remove_entry(&path, true, cancel)?;
            on_removed(&path);
            continue;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !metadata.is_dir() {
            remove_entry(&path, false, cancel)?;
            on_removed(&path);
            continue;
        }
        let mut files = Vec::new();
        let mut directories = Vec::new();
        for entry in std::fs::read_dir(&path)? {
            check_cancelled(cancel)?;
            let entry = entry?;
            // DirEntry::file_type does not follow symlinks and can reuse the
            // directory listing's type information, avoiding per-file stat calls.
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => directories.push(entry.path()),
                Ok(_) => files.push(entry.path()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        remove_files(
            &files,
            cancel,
            |file| remove_entry(file, false, cancel),
            &mut on_removed,
        )?;
        pending.push((path, true));
        pending.extend(directories.into_iter().map(|directory| (directory, false)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, Mutex};

    #[test]
    fn requests_are_parallel_bounded_and_all_reported() {
        let files: Vec<_> = (0..32).map(|i| PathBuf::from(i.to_string())).collect();
        let barrier = Barrier::new(WORKERS);
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let mut removed = Vec::new();
        remove_files(
            &files,
            &AtomicBool::new(false),
            |_| {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                barrier.wait();
                active.fetch_sub(1, Ordering::SeqCst);
                // Keep batches aligned so the test does not depend on timing.
                barrier.wait();
                Ok(())
            },
            &mut |path| removed.push(path.to_path_buf()),
        )
        .unwrap();
        assert_eq!(peak.load(Ordering::SeqCst), WORKERS);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        removed.sort();
        let mut expected = files;
        expected.sort();
        assert_eq!(removed, expected);
    }

    #[test]
    fn cancelled_delete_starts_no_requests() {
        let paths = vec![PathBuf::from("untouched")];
        let result = remove_files(
            &paths,
            &AtomicBool::new(true),
            |_| panic!("must not delete"),
            &mut |_| panic!("must not report success"),
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn failures_stop_scheduling_and_join_started_requests() {
        let files: Vec<_> = (0..1000).map(|i| PathBuf::from(i.to_string())).collect();
        let started = AtomicUsize::new(0);
        let completed = Mutex::new(Vec::new());
        let mut removed = Vec::new();
        let result = remove_files(
            &files,
            &AtomicBool::new(false),
            |path| {
                let index = started.fetch_add(1, Ordering::SeqCst);
                if index == 0 {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
                }
                completed.lock().unwrap().push(path.to_path_buf());
                Ok(())
            },
            &mut |path| removed.push(path.to_path_buf()),
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert!(started.load(Ordering::SeqCst) < files.len());
        removed.sort();
        let mut expected = completed.into_inner().unwrap();
        expected.sort();
        assert_eq!(removed, expected);
    }

    #[test]
    fn removes_nested_tree_without_following_symlinks() {
        let root = std::env::temp_dir().join(format!(
            "dualbeam-nfs-delete-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let target = root.join("delete");
        let outside = root.join("keep");
        std::fs::create_dir_all(target.join("nested/empty")).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("safe"), b"keep").unwrap();
        for i in 0..40 {
            std::fs::write(target.join(format!("nested/{i}")), b"delete").unwrap();
        }
        std::os::unix::fs::symlink(&outside, target.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("missing"), target.join("dangling")).unwrap();
        let mut removed = Vec::new();
        remove_tree(&target, &AtomicBool::new(false), |path| {
            removed.push(path.to_path_buf())
        })
        .unwrap();
        assert!(!target.exists());
        assert_eq!(std::fs::read(outside.join("safe")).unwrap(), b"keep");
        assert_eq!(removed.len(), 45);
        assert_eq!(removed.last(), Some(&target));
        std::fs::remove_dir_all(root).unwrap();
    }
}
