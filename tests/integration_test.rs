use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use zip::ZipArchive;

/// Test helper to get the path to the runzip binary
fn get_runzip_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("Failed to get current executable path");
    path.pop(); // Remove test executable name
    if path.ends_with("deps") {
        path.pop(); // Remove "deps" directory
    }
    path.push("runzip");
    path
}

/// Test helper to copy test archives to a temporary directory
fn setup_test_archives(temp_dir: &Path) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let originals_dir = Path::new("tests/originals");
    let windows_src = originals_dir.join("windows-archive.zip");
    let mac_src = originals_dir.join("mac-archive.zip");
    let linux_src = originals_dir.join("linux-archive.zip");

    let windows_dst = temp_dir.join("windows-archive.zip");
    let mac_dst = temp_dir.join("mac-archive.zip");
    let linux_dst = temp_dir.join("linux-archive.zip");

    fs::copy(&windows_src, &windows_dst)?;
    fs::copy(&mac_src, &mac_dst)?;
    fs::copy(&linux_src, &linux_dst)?;

    Ok((windows_dst, mac_dst, linux_dst))
}

/// Test helper to extract filenames from a ZIP archive
fn extract_filenames_from_zip(zip_path: &Path) -> Result<Vec<Vec<u8>>> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;
    let mut filenames = Vec::new();

    for i in 0..archive.len() {
        let file_entry = archive.by_index_raw(i)?;
        filenames.push(file_entry.name_raw().to_vec());
    }

    Ok(filenames)
}

/// Test helper to check if bytes contain valid UTF-8 Russian text
fn is_valid_utf8_russian(bytes: &[u8]) -> bool {
    if let Ok(utf8_str) = std::str::from_utf8(bytes) {
        // Check if it contains Cyrillic characters (Russian alphabet range)
        utf8_str.chars().any(|c| {
            matches!(c,
                '\u{0400}'..='\u{04FF}' |  // Cyrillic
                '\u{0500}'..='\u{052F}'    // Cyrillic Supplement
            )
        })
    } else {
        false
    }
}

/// Test helper to check if bytes look like corrupted encoding (contains replacement chars or non-printable chars)
fn looks_like_encoding_corruption(bytes: &[u8]) -> bool {
    if let Ok(utf8_str) = std::str::from_utf8(bytes) {
        // Look for replacement characters or sequences of strange characters
        utf8_str.chars().any(|c| {
            c == '\u{FFFD}' ||  // Replacement character
            (c as u32 > 127 && !matches!(c, '\u{0400}'..='\u{04FF}' | '\u{0500}'..='\u{052F}'))
        }) && utf8_str.contains('\u{FFFD}') // Contains replacement characters which often indicate encoding issues
    } else {
        // If it's not valid UTF-8, it might be corrupted encoding
        true
    }
}

/// Test helper to run runzip on files and capture output
fn run_runzip(binary_path: &Path, zip_files: &[&Path]) -> Result<std::process::Output> {
    let mut cmd = Command::new(binary_path);
    cmd.args(zip_files.iter().map(|p| p.as_os_str()));

    let output = cmd.output()?;
    Ok(output)
}

/// Test helper to run runzip in dry-run mode
fn run_runzip_dry_run(binary_path: &Path, zip_files: &[&Path]) -> Result<std::process::Output> {
    let mut cmd = Command::new(binary_path);
    cmd.arg("--dry-run");
    cmd.args(zip_files.iter().map(|p| p.as_os_str()));

    let output = cmd.output()?;
    Ok(output)
}

#[test]
fn test_windows_archive_has_encoding_issues() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let (windows_zip, _, _) = setup_test_archives(temp_dir.path())?;

    let filenames = extract_filenames_from_zip(&windows_zip)?;

    // The Windows archive should have at least one filename with encoding issues
    let has_corrupted_filename = filenames.iter().any(|filename| {
        looks_like_encoding_corruption(filename)
            || (!is_valid_utf8_russian(filename)
                && !std::str::from_utf8(filename).map_or(false, |s| s.is_ascii()))
    });

    assert!(
        has_corrupted_filename,
        "Windows archive should contain filenames with encoding issues"
    );

    Ok(())
}

#[test]
fn test_mac_archive_has_proper_utf8() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let (_, mac_zip, _) = setup_test_archives(temp_dir.path())?;

    let filenames = extract_filenames_from_zip(&mac_zip)?;

    // The Mac archive should have proper UTF-8 Russian filenames
    let has_proper_russian = filenames
        .iter()
        .any(|filename| is_valid_utf8_russian(filename));

    assert!(
        has_proper_russian,
        "Mac archive should contain proper UTF-8 Russian filenames"
    );

    Ok(())
}

#[test]
fn test_linux_archive_has_utf8_flag() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let (_, _, linux_zip) = setup_test_archives(temp_dir.path())?;
    let binary_path = get_runzip_binary();

    // Run runzip on the Linux archive in dry-run mode
    let output = run_runzip_dry_run(&binary_path, &[&linux_zip])?;

    // Verify the command succeeded
    assert!(
        output.status.success(),
        "Dry run should succeed on Linux archive. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify output indicates files are already UTF-8
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("OK (already UTF-8)"),
        "Linux archive should be recognized as already having UTF-8 flag. Output: {}",
        stdout
    );

    Ok(())
}

#[test]
fn test_dry_run_mode() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let (windows_zip, mac_zip, linux_zip) = setup_test_archives(temp_dir.path())?;
    let binary_path = get_runzip_binary();

    // Get original modification times
    let windows_original_mtime = fs::metadata(&windows_zip)?.modified()?;
    let mac_original_mtime = fs::metadata(&mac_zip)?.modified()?;
    let linux_original_mtime = fs::metadata(&linux_zip)?.modified()?;

    // Run in dry-run mode
    let output = run_runzip_dry_run(&binary_path, &[&windows_zip, &mac_zip, &linux_zip])?;

    // Verify the command succeeded
    assert!(
        output.status.success(),
        "Dry run should succeed. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify files were not modified
    let windows_new_mtime = fs::metadata(&windows_zip)?.modified()?;
    let mac_new_mtime = fs::metadata(&mac_zip)?.modified()?;
    let linux_new_mtime = fs::metadata(&linux_zip)?.modified()?;

    assert_eq!(
        windows_original_mtime, windows_new_mtime,
        "Windows archive should not be modified in dry-run mode"
    );
    assert_eq!(
        mac_original_mtime, mac_new_mtime,
        "Mac archive should not be modified in dry-run mode"
    );
    assert_eq!(
        linux_original_mtime, linux_new_mtime,
        "Linux archive should not be modified in dry-run mode"
    );

    // Verify output contains expected information
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("windows-archive.zip"),
        "Output should mention windows archive"
    );
    assert!(
        stdout.contains("mac-archive.zip"),
        "Output should mention mac archive"
    );
    assert!(
        stdout.contains("linux-archive.zip"),
        "Output should mention linux archive"
    );

    Ok(())
}

#[test]
fn test_fixing_windows_archive() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let (windows_zip, _, _) = setup_test_archives(temp_dir.path())?;
    let binary_path = get_runzip_binary();

    // Get original filenames
    let original_filenames = extract_filenames_from_zip(&windows_zip)?;

    // Run runzip (not dry-run)
    let output = run_runzip(&binary_path, &[&windows_zip])?;

    // Verify the command succeeded
    assert!(
        output.status.success(),
        "runzip should succeed. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Get new filenames after processing
    let new_filenames = extract_filenames_from_zip(&windows_zip)?;

    // Verify we have the same number of files
    assert_eq!(
        original_filenames.len(),
        new_filenames.len(),
        "Number of files should remain the same"
    );

    // Verify that at least one filename changed (was fixed)
    let filenames_changed = original_filenames
        .iter()
        .zip(new_filenames.iter())
        .any(|(orig, new)| orig != new);

    // We expect changes if there were encoding issues
    let had_encoding_issues = original_filenames.iter().any(|filename| {
        looks_like_encoding_corruption(filename)
            || (!is_valid_utf8_russian(filename)
                && !std::str::from_utf8(filename).map_or(false, |s| s.is_ascii()))
    });

    if had_encoding_issues {
        assert!(
            filenames_changed,
            "Filenames should be fixed when encoding issues exist"
        );
    }

    // Verify all new filenames are valid UTF-8
    for filename in &new_filenames {
        assert!(
            std::str::from_utf8(filename).is_ok(),
            "All processed filenames should be valid UTF-8"
        );
    }

    Ok(())
}

#[test]
fn test_fixing_mac_archive() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let (_, mac_zip, _) = setup_test_archives(temp_dir.path())?;
    let binary_path = get_runzip_binary();

    // Get original filenames from the Mac archive (should already be proper UTF-8)
    let original_filenames = extract_filenames_from_zip(&mac_zip)?;

    // Run runzip on the Mac archive
    let output = run_runzip(&binary_path, &[&mac_zip])?;

    // Verify the command succeeded
    assert!(
        output.status.success(),
        "runzip should succeed on Mac archive. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Get new filenames after processing
    let new_filenames = extract_filenames_from_zip(&mac_zip)?;

    // Verify we have the same number of files
    assert_eq!(
        original_filenames.len(),
        new_filenames.len(),
        "Number of files should remain the same"
    );

    // Verify filenames are unchanged (since they were already proper UTF-8)
    for (orig, new) in original_filenames.iter().zip(new_filenames.iter()) {
        assert_eq!(
            orig, new,
            "Mac archive filenames should remain unchanged (already proper UTF-8)"
        );
    }

    // Verify all filenames are still valid UTF-8
    for filename in &new_filenames {
        assert!(
            std::str::from_utf8(filename).is_ok(),
            "All filenames should remain valid UTF-8"
        );
    }

    // Verify output indicates no changes were needed
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("OK"),
        "Output should indicate files are OK (no conversion needed)"
    );

    Ok(())
}

#[test]
fn test_archives_remain_extractable() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let (windows_zip, mac_zip, linux_zip) = setup_test_archives(temp_dir.path())?;
    let binary_path = get_runzip_binary();

    // Process the archives
    let output = run_runzip(&binary_path, &[&windows_zip, &mac_zip, &linux_zip])?;
    assert!(output.status.success(), "runzip should succeed");

    // Test that we can still extract both archives
    let extract_dir = temp_dir.path().join("extracted");
    fs::create_dir(&extract_dir)?;

    // Test Windows archive extraction
    let windows_extract_dir = extract_dir.join("windows");
    fs::create_dir(&windows_extract_dir)?;

    let extract_output = Command::new("unzip")
        .arg("-q") // Quiet mode
        .arg(&windows_zip)
        .arg("-d")
        .arg(&windows_extract_dir)
        .output()?;

    assert!(
        extract_output.status.success(),
        "Processed Windows archive should be extractable. stderr: {}",
        String::from_utf8_lossy(&extract_output.stderr)
    );

    // Test Mac archive extraction
    let mac_extract_dir = extract_dir.join("mac");
    fs::create_dir(&mac_extract_dir)?;

    let extract_output = Command::new("unzip")
        .arg("-q") // Quiet mode
        .arg(&mac_zip)
        .arg("-d")
        .arg(&mac_extract_dir)
        .output()?;

    assert!(
        extract_output.status.success(),
        "Processed Mac archive should be extractable. stderr: {}",
        String::from_utf8_lossy(&extract_output.stderr)
    );

    // Test Linux archive extraction
    let linux_extract_dir = extract_dir.join("linux");
    fs::create_dir(&linux_extract_dir)?;

    let extract_output = Command::new("unzip")
        .arg("-q") // Quiet mode
        .arg(&linux_zip)
        .arg("-d")
        .arg(&linux_extract_dir)
        .output()?;

    assert!(
        extract_output.status.success(),
        "Processed Linux archive should be extractable. stderr: {}",
        String::from_utf8_lossy(&extract_output.stderr)
    );

    // Verify extracted files exist
    let windows_entries: Vec<_> = fs::read_dir(&windows_extract_dir)?.collect();
    let mac_entries: Vec<_> = fs::read_dir(&mac_extract_dir)?.collect();
    let linux_entries: Vec<_> = fs::read_dir(&linux_extract_dir)?.collect();

    assert!(
        !windows_entries.is_empty(),
        "Windows archive should contain extractable files"
    );
    assert!(
        !mac_entries.is_empty(),
        "Mac archive should contain extractable files"
    );
    assert!(
        !linux_entries.is_empty(),
        "Linux archive should contain extractable files"
    );

    Ok(())
}

#[test]
fn test_verbose_output() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let (windows_zip, _, _) = setup_test_archives(temp_dir.path())?;
    let binary_path = get_runzip_binary();

    // Run with verbose output
    let output = Command::new(&binary_path)
        .arg("-vv") // Very verbose
        .arg("--dry-run")
        .arg(&windows_zip)
        .output()?;

    assert!(output.status.success(), "Verbose dry run should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Verify verbose output contains encoding information
    assert!(
        stdout.contains("Raw bytes")
            || stdout.contains("detected")
            || stdout.contains("Converting"),
        "Verbose output should contain encoding detection information"
    );

    Ok(())
}

#[test]
fn test_nonexistent_file_handling() -> Result<()> {
    let binary_path = get_runzip_binary();

    let output = Command::new(&binary_path).arg("nonexistent.zip").output()?;

    // Should fail with appropriate error
    assert!(
        !output.status.success(),
        "Should fail when file doesn't exist"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error") || stderr.contains("Failed"),
        "Should output error message"
    );

    Ok(())
}
