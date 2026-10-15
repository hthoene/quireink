//! Copying a file from one folder storage to another mounted beside it,
//! in parts, as moving an upload into the shared storage does.

use quireink_fs::DirStorage;
use quireink_platform::{Mounted, Storage};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("quireink-mounted-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn large_files_cross_mount_points_whole() {
    let (base, shared) = (temp("base"), temp("shared"));
    let storage = Mounted::new(
        DirStorage::new(&base).unwrap(),
        "storage",
        DirStorage::new(&shared).unwrap(),
    );
    let data: Vec<u8> = (0..100_000u32).map(|i| (i * 7 % 251) as u8).collect();
    storage.write(".quireink/tmp/request-1", &[]).unwrap();
    for part in data.chunks(65_536) {
        storage.append(".quireink/tmp/request-1", part).unwrap();
    }
    // As `files::copy` does.
    let (from, to) = (".quireink/tmp/request-1", "storage/Pictures/a.png");
    storage.write(to, &[]).unwrap();
    let mut chunk = vec![0u8; 16 * 1024];
    let mut at = 0u64;
    loop {
        let n = storage.read_at(from, at, &mut chunk).unwrap();
        if n == 0 {
            break;
        }
        storage.append(to, &chunk[..n]).unwrap();
        at += n as u64;
    }
    let body = std::fs::read(base.join(".quireink/tmp/request-1")).unwrap();
    let copied = std::fs::read(shared.join("Pictures/a.png")).unwrap();
    let first = |a: &[u8]| a.iter().zip(&data).position(|(x, y)| x != y);
    assert!(
        body == data && copied == data,
        "body {} bytes, differs at {:?}; copy {} bytes, differs at {:?}",
        body.len(),
        first(&body),
        copied.len(),
        first(&copied)
    );
}
