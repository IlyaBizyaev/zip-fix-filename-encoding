#![warn(clippy::pedantic)]

use anyhow::{Context, Result, anyhow};
use chardetng::EncodingDetector;
use clap::Parser;
use encoding_rs::{Encoding, IBM866, KOI8_R, KOI8_U, UTF_8, WINDOWS_1251};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use zip::write::FileOptions;
use zip::{HasZipMetadata, ZipArchive, ZipWriter};

#[derive(Parser)]
#[command(
    name = "runzip",
    version = "2.0.0",
    about = "Russian filename encoding fix inside ZIP archives",
    long_about = "Convert filenames inside ZIP archives from older Russian encodings\n(koi8-r, koi8-u, cp866, windows-1251) to UTF-8."
)]
struct Args {
    /// Dry run. Do not modify the <file.zip>
    #[arg(short = 'n', long = "dry-run")]
    dry_run: bool,

    /// Verbose output (can be repeated)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    verbose: u8,

    /// Set source encoding. Auto-detect, if not set
    #[arg(short = 's', long = "source")]
    source_encoding: Option<String>,

    /// ZIP files to process
    files: Vec<PathBuf>,
}

fn convert_encoding(
    text: &[u8],
    from_encoding: &'static Encoding,
    to_encoding: &'static Encoding,
) -> Result<Vec<u8>> {
    // First, decode from source encoding
    let (decoded, _, had_errors) = from_encoding.decode(text);
    if had_errors {
        return Err(anyhow!("Failed to decode from {}", from_encoding.name()));
    }

    // Then encode to target encoding
    let (encoded, _, had_errors) = to_encoding.encode(&decoded);
    if had_errors {
        return Err(anyhow!("Failed to encode to {}", to_encoding.name()));
    }

    Ok(encoded.into_owned())
}

/// Check if we should attempt encoding detection for this ZIP file entry
fn should_check_encoding<R: Read>(zip_file: &zip::read::ZipFile<R>) -> bool {
    // If the EFS flag indicates UTF-8, we don't need to recode.
    !zip_file.get_metadata().is_utf8
}

/// Check if filename is valid UTF-8 with Cyrillic content
fn is_valid_utf8_cyrillic(filename: &[u8]) -> bool {
    if let Ok(utf8_str) = std::str::from_utf8(filename) {
        // Only consider it valid UTF-8 if it contains Cyrillic characters
        utf8_str
            .chars()
            .any(|c| matches!(c, '\u{0400}'..='\u{04FF}' | '\u{0500}'..='\u{052F}'))
    } else {
        false
    }
}

fn detect_cyrillic_encoding(filename: &[u8], verbose: u8) -> &'static Encoding {
    // First, check if the filename is already valid UTF-8 with Cyrillic content
    if is_valid_utf8_cyrillic(filename) {
        if verbose >= 1 {
            println!("For filename detection:");
            println!("\tAlready valid UTF-8 with Cyrillic content");
        }
        return UTF_8;
    }

    // Check for pure ASCII (which is also valid UTF-8)
    if let Ok(utf8_str) = std::str::from_utf8(filename)
        && !utf8_str.chars().any(|c| c as u32 > 127)
    {
        if verbose >= 1 {
            println!("For filename detection:");
            println!("\tPure ASCII, treating as UTF-8");
        }
        return UTF_8;
    }

    // Use chardetng for encoding detection
    let mut detector = EncodingDetector::new();
    detector.feed(filename, true);
    let detected_encoding = detector.guess(None, true);

    if verbose >= 1 {
        println!("For filename detection:");
        println!("\tchardetng detected: {}", detected_encoding.name());
    }

    // Check if the detected encoding is one of our supported encodings
    if detected_encoding == UTF_8
        || detected_encoding == WINDOWS_1251
        || detected_encoding == IBM866
        || detected_encoding == KOI8_R
        || detected_encoding == KOI8_U
    {
        detected_encoding
    } else {
        // For unsupported encodings, default to UTF-8 (maintains original behavior)
        if verbose >= 1 {
            println!("\tUnsupported encoding detected, defaulting to UTF-8");
        }
        UTF_8
    }
}

/// Convert a string encoding name to the corresponding `encoding_rs` Encoding
fn string_to_encoding(encoding_name: &str) -> Result<&'static Encoding> {
    match encoding_name.to_lowercase().as_str() {
        "utf-8" | "utf-8-mac" => Ok(UTF_8), // Treat UTF-8-MAC as UTF-8 for simplicity
        "windows-1251" => Ok(WINDOWS_1251),
        "cp866" => Ok(IBM866),
        "koi8-r" => Ok(KOI8_R),
        "koi8-u" => Ok(KOI8_U),
        _ => Err(anyhow!("Unsupported encoding: {encoding_name}")),
    }
}

fn process_file_dry_run<R: Read>(
    file_entry: &zip::read::ZipFile<R>,
    source_encoding: Option<&'static Encoding>,
    verbose: u8,
) {
    let filename_bytes = file_entry.name_raw();
    let filename_display = String::from_utf8_lossy(filename_bytes);

    if verbose >= 2 {
        println!("Raw bytes for '{filename_display}': {filename_bytes:02x?}");
    }

    // Check if we should process this file (skip if EFS flag indicates UTF-8)
    if !should_check_encoding(file_entry) {
        println!("  {filename_display}: OK (already UTF-8)");
        return;
    }

    let detected_encoding =
        source_encoding.unwrap_or_else(|| detect_cyrillic_encoding(filename_bytes, verbose));

    if detected_encoding == UTF_8 {
        println!("  {filename_display}: OK");
    } else {
        if verbose >= 1 {
            println!(
                "  Converting \"{filename_display}\" ({} -> UTF-8)",
                detected_encoding.name()
            );
        }

        match convert_encoding(filename_bytes, detected_encoding, UTF_8) {
            Ok(new_name_bytes) => {
                let new_name = String::from_utf8_lossy(&new_name_bytes);
                if filename_bytes.len() == new_name_bytes.len() && filename_bytes == new_name_bytes
                {
                    println!("  {filename_display}: OK");
                } else {
                    println!(
                        "  {new_name}: WOULD FIX ({} -> UTF-8)",
                        detected_encoding.name()
                    );
                }
            }
            Err(e) => {
                println!("  Failed to recode \"{filename_display}\": {e}");
            }
        }
    }
}

fn copy_file_to_archive<R: Read, W: Write + std::io::Seek>(
    mut file_entry: zip::read::ZipFile<R>,
    zip_writer: &mut ZipWriter<W>,
    new_filename_bytes: &[u8],
) -> Result<()> {
    let mut options = FileOptions::<()>::default().compression_method(file_entry.compression());

    // Set proper permissions for directories
    if let Some(perms) = file_entry.unix_mode() {
        options = options.unix_permissions(perms);
    } else if new_filename_bytes.ends_with(b"/") {
        // Default directory permissions: 755 (rwxr-xr-x)
        options = options.unix_permissions(0o755);
    }

    let new_filename = String::from_utf8_lossy(new_filename_bytes);
    zip_writer
        .start_file(&new_filename, options)
        .context("Failed to start file in new archive")?;

    let mut buffer = Vec::new();
    file_entry
        .read_to_end(&mut buffer)
        .context("Failed to read file contents")?;
    zip_writer
        .write_all(&buffer)
        .context("Failed to write file contents")?;

    Ok(())
}

fn process_file_write<R: Read, W: Write + std::io::Seek>(
    file_entry: zip::read::ZipFile<R>,
    zip_writer: &mut ZipWriter<W>,
    source_encoding: Option<&'static Encoding>,
    verbose: u8,
) -> Result<()> {
    let filename_bytes = file_entry.name_raw().to_vec();
    let filename_display = String::from_utf8_lossy(&filename_bytes);

    // Check if we should process this file (skip if EFS flag indicates UTF-8)
    if !should_check_encoding(&file_entry) {
        println!("  {filename_display}: OK (already UTF-8)");
        copy_file_to_archive(file_entry, zip_writer, &filename_bytes)?;
        return Ok(());
    }

    let detected_encoding =
        source_encoding.unwrap_or_else(|| detect_cyrillic_encoding(&filename_bytes, verbose));

    let new_filename_bytes = if detected_encoding == UTF_8 {
        println!("  {filename_display}: OK");
        filename_bytes.clone()
    } else {
        if verbose >= 1 {
            println!(
                "  Converting \"{filename_display}\" ({} -> UTF-8)",
                detected_encoding.name()
            );
        }

        match convert_encoding(&filename_bytes, detected_encoding, UTF_8) {
            Ok(new_name_bytes) => {
                let new_name = String::from_utf8_lossy(&new_name_bytes);
                if filename_bytes.len() == new_name_bytes.len() && filename_bytes == new_name_bytes
                {
                    println!("  {filename_display}: OK");
                    filename_bytes.clone()
                } else {
                    println!(
                        "  {new_name}: FIXED ({} -> UTF-8)",
                        detected_encoding.name()
                    );
                    new_name_bytes
                }
            }
            Err(e) => {
                println!("  Failed to recode \"{filename_display}\": {e}");
                filename_bytes.clone()
            }
        }
    };

    copy_file_to_archive(file_entry, zip_writer, &new_filename_bytes)?;
    Ok(())
}

fn fix_cyrillic_filenames(
    zipfile: &Path,
    dry_run: bool,
    source_encoding: Option<&'static Encoding>,
    verbose: u8,
) -> Result<()> {
    let file = File::open(zipfile).context(format!("Failed to open {}", zipfile.display()))?;
    let mut archive = ZipArchive::new(file).context("Failed to read ZIP archive")?;

    let file_count = archive.len();
    println!(
        "{} contains {} file{}",
        zipfile.display(),
        file_count,
        if file_count == 1 { "" } else { "s" }
    );

    if dry_run {
        // For dry run, just analyze without modifying
        for i in 0..file_count {
            let file_entry = archive
                .by_index_raw(i)
                .context("Failed to read file entry")?;
            process_file_dry_run(&file_entry, source_encoding, verbose);
        }
    } else {
        // For actual modification, we need to create a new archive
        let temp_file = NamedTempFile::new_in(zipfile.parent().unwrap_or_else(|| Path::new(".")))
            .context("Failed to create temporary file")?;
        let mut zip_writer = ZipWriter::new(&temp_file);

        for i in 0..file_count {
            let file_entry = archive.by_index_raw(i).context("Failed to read file entry")?;
            process_file_write(file_entry, &mut zip_writer, source_encoding, verbose)?;
        }

        zip_writer
            .finish()
            .context("Failed to finalize new archive")?;
        drop(archive); // Close the original file

        // Atomically replace original with modified version
        temp_file
            .persist(zipfile)
            .context("Failed to replace original file with modified version")?;
    }

    Ok(())
}

fn main() {
    let args = Args::parse();

    if args.files.is_empty() {
        eprintln!("Error: No ZIP files specified");
        std::process::exit(1);
    }

    let source_encoding = if let Some(ref source) = args.source_encoding {
        if let Ok(encoding) = string_to_encoding(source) {
            Some(encoding)
        } else {
            eprintln!("Error: Invalid source encoding: {source}");
            std::process::exit(1);
        }
    } else {
        None
    };

    for zipfile in &args.files {
        if let Err(e) = fix_cyrillic_filenames(zipfile, args.dry_run, source_encoding, args.verbose)
        {
            eprintln!("Error processing {}: {e}", zipfile.display());
            std::process::exit(1);
        }
    }
}
