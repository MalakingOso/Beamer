//! Tests for `archive`: zip-slip rejection, and the swap / commit / rollback /
//! sweep cycle that keeps a last-good runtime dir on disk.

use std::io::Write;

use super::*;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("beamer-archive-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, bytes) in entries {
        w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn a_flat_zip_extracts_every_entry() {
    let dir = temp_dir("flat");
    let zip = dir.join("rt.zip");
    make_zip(&zip, &[("llama-server.exe", b"exe"), ("ggml.dll", b"dll")]);
    let into = dir.join("rt.staging");
    extract(&zip, &into).unwrap();
    assert_eq!(read(&into.join("llama-server.exe")), "exe");
    assert_eq!(read(&into.join("ggml.dll")), "dll");
}

#[test]
fn a_parent_dir_entry_rejects_the_whole_archive() {
    let dir = temp_dir("slip");
    let zip = dir.join("evil.zip");
    make_zip(&zip, &[("ok.dll", b"fine"), ("../evil.dll", b"pwned")]);
    let into = dir.join("rt.staging");
    assert!(extract(&zip, &into).is_err());
    assert!(!dir.join("evil.dll").exists(), "nothing written outside the staging dir");
    assert!(!into.exists(), "a rejected archive leaves no partial staging dir");
}

#[test]
fn an_absolute_entry_rejects_the_whole_archive() {
    let dir = temp_dir("absolute");
    let zip = dir.join("abs.zip");
    make_zip(&zip, &[("/tmp/beamer-archive-test-absolute.dll", b"pwned")]);
    assert!(extract(&zip, &dir.join("rt.staging")).is_err());
}

#[test]
fn swap_in_replaces_the_dir_and_parks_the_old_one_until_commit() {
    let dir = temp_dir("swap");
    let dest = dir.join("rt");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("v"), "old").unwrap();
    let staging = staging_path(&dest);
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("v"), "new").unwrap();

    swap_in(&staging, &dest).unwrap();
    assert_eq!(read(&dest.join("v")), "new");
    assert_eq!(read(&old_path(&dest).join("v")), "old");
    assert!(!staging.exists());

    commit(&dest);
    assert!(!old_path(&dest).exists());
}

#[test]
fn rollback_restores_the_old_dir() {
    let dir = temp_dir("rollback");
    let dest = dir.join("rt");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("v"), "old").unwrap();
    let staging = staging_path(&dest);
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("v"), "new").unwrap();

    swap_in(&staging, &dest).unwrap();
    rollback(&dest).unwrap();
    assert_eq!(read(&dest.join("v")), "old");
    assert!(!old_path(&dest).exists());
}

#[test]
fn swap_in_with_no_previous_dir_just_moves_staging() {
    let dir = temp_dir("fresh");
    let dest = dir.join("rt");
    let staging = staging_path(&dest);
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("v"), "new").unwrap();
    swap_in(&staging, &dest).unwrap();
    assert_eq!(read(&dest.join("v")), "new");
    rollback(&dest).unwrap();
    assert!(dest.exists(), "nothing parked means nothing to roll back to");
}

#[test]
fn sweep_restores_an_old_dir_orphaned_mid_swap() {
    let dir = temp_dir("sweep-orphan");
    let dest = dir.join("rt");
    std::fs::create_dir_all(old_path(&dest)).unwrap();
    std::fs::write(old_path(&dest).join("v"), "last good").unwrap();
    sweep(&dest);
    assert_eq!(read(&dest.join("v")), "last good");
    assert!(!old_path(&dest).exists());
}

#[test]
fn sweep_clears_leftovers_when_the_dir_is_intact() {
    let dir = temp_dir("sweep-leftovers");
    let dest = dir.join("rt");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::create_dir_all(old_path(&dest)).unwrap();
    std::fs::create_dir_all(staging_path(&dest)).unwrap();
    std::fs::write(zip_part_path(&dest), "partial").unwrap();
    sweep(&dest);
    assert!(dest.exists());
    assert!(!old_path(&dest).exists());
    assert!(!staging_path(&dest).exists());
    assert!(!zip_part_path(&dest).exists());
}
