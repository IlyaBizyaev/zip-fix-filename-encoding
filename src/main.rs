use anyhow::{Context, Result, anyhow};
use chardetng::EncodingDetector;
use clap::Parser;
use encoding_rs::*;
use std::fs::File;
use std::io::{Read, Write};
use zip::write::FileOptions;
use zip::{ZipArchive, ZipWriter};

#[derive(Parser)]
#[command(
    name = "runzip",
    version = "2.0.0",
    about = "Russian filename encoding fix inside ZIP archives",
    long_about = "Convert filenames inside ZIP archives from autodetected older Russian encodings\n(koi8-r, koi8-u, cp866, windows-1251) to UTF-8."
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

    /// Set target encoding. Default is UTF-8
    #[arg(short = 't', long = "target", default_value = "utf-8")]
    target_encoding: String,

    /// Make archive readable on Windows (reverse operation)
    /// NOTE: -w implies -t cp866 (Yes! MS-DOS!)
    #[arg(short = 'w', long = "windows")]
    windows_mode: bool,

    /// ZIP files to process
    files: Vec<String>,
}

fn convert_encoding(text: &[u8], from_encoding: &str, to_encoding: &str) -> Result<Vec<u8>> {
    // First, decode from source encoding
    let source_encoding = match from_encoding.to_lowercase().as_str() {
        "utf-8" => UTF_8,
        "utf-8-mac" => UTF_8, // Treat UTF-8-MAC as UTF-8 for simplicity
        "windows-1251" => WINDOWS_1251,
        "cp866" => IBM866,
        "koi8-r" => KOI8_R,
        "koi8-u" => KOI8_U,
        _ => return Err(anyhow!("Unsupported source encoding: {}", from_encoding)),
    };

    let (decoded, _, had_errors) = source_encoding.decode(text);
    if had_errors {
        return Err(anyhow!("Failed to decode from {}", from_encoding));
    }

    // Then encode to target encoding
    let target_encoding_obj = match to_encoding.to_lowercase().as_str() {
        "utf-8" => UTF_8,
        "windows-1251" => WINDOWS_1251,
        "cp866" => IBM866,
        "koi8-r" => KOI8_R,
        "koi8-u" => KOI8_U,
        _ => return Err(anyhow!("Unsupported target encoding: {}", to_encoding)),
    };

    let (encoded, _, had_errors) = target_encoding_obj.encode(&decoded);
    if had_errors {
        return Err(anyhow!("Failed to encode to {}", to_encoding));
    }

    Ok(encoded.into_owned())
}

fn detect_cyrillic_encoding(filename: &[u8], verbose: u8) -> &'static str {
    // First, check if the filename is already valid UTF-8 with Cyrillic content
    if let Ok(utf8_str) = std::str::from_utf8(filename) {
        // Check if it contains Cyrillic characters
        let has_cyrillic = utf8_str
            .chars()
            .any(|c| matches!(c, '\u{0400}'..='\u{04FF}' | '\u{0500}'..='\u{052F}'));

        if has_cyrillic || !utf8_str.chars().any(|c| c as u32 > 127) {
            // Either has Cyrillic or is pure ASCII - both are fine as UTF-8
            if verbose >= 1 {
                println!("For filename detection:");
                println!("\tAlready valid UTF-8 with Cyrillic content or ASCII");
            }
            return "UTF-8";
        }
    }

    // Use chardetng for encoding detection
    let mut detector = EncodingDetector::new();
    detector.feed(filename, true);
    let detected_encoding = detector.guess(None, true);

    if verbose >= 1 {
        println!("For filename detection:");
        println!("\tchardetng detected: {}", detected_encoding.name());
    }

    // Map detected encoding to our supported set
    match detected_encoding.name() {
        "UTF-8" => "UTF-8",
        "windows-1251" => "windows-1251",
        "IBM866" => "CP866",
        "KOI8-R" => "KOI8-R",
        "KOI8-U" => "KOI8-U",
        // For unsupported encodings, default to UTF-8 (maintains original behavior)
        _ => {
            if verbose >= 1 {
                println!("\tUnsupported encoding detected, defaulting to UTF-8");
            }
            "UTF-8"
        }
    }
}

fn fix_cyrillic_filenames(
    zipfile: &str,
    dry_run: bool,
    source_encoding: Option<&str>,
    target_encoding: &str,
    verbose: u8,
) -> Result<()> {
    let file = File::open(zipfile).context(format!("Failed to open {}", zipfile))?;
    let mut archive = ZipArchive::new(file).context("Failed to read ZIP archive")?;

    let file_count = archive.len();
    println!(
        "{} contains {} file{}",
        zipfile,
        file_count,
        if file_count == 1 { "" } else { "s" }
    );

    if dry_run {
        // For dry run, just analyze without modifying
        for i in 0..file_count {
            let file_entry = archive
                .by_index_raw(i)
                .context("Failed to read file entry")?;
            let filename_bytes = file_entry.name_raw();
            let filename_display = String::from_utf8_lossy(filename_bytes);

            if verbose >= 2 {
                println!(
                    "Raw bytes for '{}': {:02x?}",
                    filename_display, filename_bytes
                );
            }

            let detected_encoding = source_encoding
                .unwrap_or_else(|| detect_cyrillic_encoding(filename_bytes, verbose));

            if detected_encoding.eq_ignore_ascii_case(target_encoding) {
                println!("  {}: OK", filename_display);
            } else {
                if verbose >= 1 {
                    println!(
                        "  Converting \"{}\" ({} -> {})",
                        filename_display, detected_encoding, target_encoding
                    );
                }

                match convert_encoding(filename_bytes, detected_encoding, target_encoding) {
                    Ok(new_name_bytes) => {
                        let new_name = String::from_utf8_lossy(&new_name_bytes);
                        if filename_bytes.len() == new_name_bytes.len()
                            && filename_bytes == new_name_bytes
                        {
                            println!("  {}: OK", filename_display);
                        } else {
                            println!(
                                "  {}: WOULD FIX ({} -> {})",
                                new_name, detected_encoding, target_encoding
                            );
                        }
                    }
                    Err(e) => {
                        println!("  Failed to recode \"{}\": {}", filename_display, e);
                    }
                }
            }
        }
    } else {
        // For actual modification, we need to create a new archive
        let temp_file = format!("{}.tmp", zipfile);
        let output_file = File::create(&temp_file).context("Failed to create temporary file")?;
        let mut zip_writer = ZipWriter::new(output_file);

        for i in 0..file_count {
            let mut file_entry = archive.by_index(i).context("Failed to read file entry")?;
            let filename_bytes = file_entry.name_raw().to_vec();
            let filename_display = String::from_utf8_lossy(&filename_bytes);

            let detected_encoding = source_encoding
                .unwrap_or_else(|| detect_cyrillic_encoding(&filename_bytes, verbose));

            let new_filename_bytes = if detected_encoding.eq_ignore_ascii_case(target_encoding) {
                println!("  {}: OK", filename_display);
                filename_bytes.clone()
            } else {
                if verbose >= 1 {
                    println!(
                        "  Converting \"{}\" ({} -> {})",
                        filename_display, detected_encoding, target_encoding
                    );
                }

                match convert_encoding(&filename_bytes, detected_encoding, target_encoding) {
                    Ok(new_name_bytes) => {
                        let new_name = String::from_utf8_lossy(&new_name_bytes);
                        if filename_bytes.len() == new_name_bytes.len()
                            && filename_bytes == new_name_bytes
                        {
                            println!("  {}: OK", filename_display);
                            filename_bytes.clone()
                        } else {
                            println!(
                                "  {}: FIXED ({} -> {})",
                                new_name, detected_encoding, target_encoding
                            );
                            new_name_bytes
                        }
                    }
                    Err(e) => {
                        println!("  Failed to recode \"{}\": {}", filename_display, e);
                        filename_bytes.clone()
                    }
                }
            };

            // Copy file with potentially new name
            let mut options =
                FileOptions::<()>::default().compression_method(file_entry.compression());

            // Set proper permissions for directories
            if let Some(perms) = file_entry.unix_mode() {
                options = options.unix_permissions(perms);
            } else if new_filename_bytes.ends_with(b"/") {
                // Default directory permissions: 755 (rwxr-xr-x)
                options = options.unix_permissions(0o755);
            }

            let new_filename = String::from_utf8_lossy(&new_filename_bytes);
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
        }

        zip_writer
            .finish()
            .context("Failed to finalize new archive")?;
        drop(archive); // Close the original file

        // Replace original with modified version
        std::fs::rename(&temp_file, zipfile)
            .context("Failed to replace original file with modified version")?;
    }

    Ok(())
}

fn main() -> Result<()> {
    let mut args = Args::parse();

    if args.files.is_empty() {
        eprintln!("Error: No ZIP files specified");
        std::process::exit(1);
    }

    // Handle Windows mode
    if args.windows_mode {
        args.target_encoding = "cp866".to_string();
    }

    // Validate encodings
    let valid_encodings = ["utf-8", "windows-1251", "cp866", "koi8-r", "koi8-u"];
    if !valid_encodings.contains(&args.target_encoding.to_lowercase().as_str()) {
        eprintln!("Error: Invalid target encoding: {}", args.target_encoding);
        std::process::exit(1);
    }

    if let Some(ref source) = args.source_encoding {
        if !valid_encodings.contains(&source.to_lowercase().as_str()) {
            eprintln!("Error: Invalid source encoding: {}", source);
            std::process::exit(1);
        }
    }

    for zipfile in &args.files {
        if let Err(e) = fix_cyrillic_filenames(
            zipfile,
            args.dry_run,
            args.source_encoding.as_deref(),
            &args.target_encoding,
            args.verbose,
        ) {
            eprintln!("Error processing {}: {}", zipfile, e);
            std::process::exit(1);
        }
    }

    Ok(())
}
