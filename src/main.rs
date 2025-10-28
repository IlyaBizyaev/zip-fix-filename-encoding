use anyhow::{anyhow, Context, Result};
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

#[derive(Debug)]
struct CharFrequencies {
    encoding: &'static str,
    characters_seen: usize,
    frequency: [f64; 256],
}

impl CharFrequencies {
    fn new(encoding: &'static str) -> Self {
        Self {
            encoding,
            characters_seen: 0,
            frequency: [0.0; 256],
        }
    }

    fn add_character(&mut self, ch: u8) {
        self.frequency[ch as usize] += 1.0;
        self.characters_seen += 1;
    }

    fn cyrillic_factor(&self) -> f64 {
        // Cyrillic character frequency scale based on Russian letter frequencies
        // From http://www.sttmedia.com/characterfrequency-cyrillic
        let mut scale = [0.0; 256];

        // ASCII characters (space to ~) get small positive weight
        for i in 32..=126 {
            scale[i] = 0.001;
        }

        // KOI8-R/KOI8-U Cyrillic characters with their frequencies
        scale[207] = 11.07; // О
        scale[197] = 8.50; // Е
        scale[193] = 7.50; // А
        scale[201] = 7.09; // И
        scale[206] = 6.70; // Н
        scale[212] = 5.97; // Т
        scale[211] = 4.97; // С
        scale[204] = 4.96; // Л
        scale[215] = 4.33; // В
        scale[210] = 4.33; // Р
        scale[203] = 3.30; // К
        scale[237] = 3.10; // М
        scale[196] = 3.09; // Д
        scale[208] = 2.47; // П
        scale[217] = 2.36; // Ы
        scale[213] = 2.22; // У
        scale[194] = 2.01; // Б
        scale[209] = 1.96; // Я
        scale[216] = 1.84; // Ь
        scale[199] = 1.72; // Г
        scale[218] = 1.48; // З
        scale[222] = 1.40; // Ч
        scale[202] = 1.21; // Й
        scale[214] = 1.01; // Ж
        scale[200] = 0.95; // Х
        scale[219] = 0.72; // Ш
        scale[192] = 0.47; // Ю
        scale[195] = 0.39; // Ц
        scale[220] = 0.35; // Э
        scale[221] = 0.30; // Щ
        scale[198] = 0.21; // Ф
        scale[163] = 0.20; // Ё
        scale[223] = 0.02; // Ъ

        // Ukrainian letters
        scale[164] = 0.3; // Є
        scale[166] = 5.0; // І
        scale[167] = 0.3; // Ї
        scale[173] = 0.01; // Ґ

        let mut factor = 0.0;
        for i in 0..256 {
            // Convert lowercase KOI8-R/KOI8-U character
            let ch = if i >= 225 { i - 32 } else { i };
            let ch = match ch {
                179 => ch - 16,                   // ё
                180 | 182 | 183 | 189 => ch - 16, // є, і, ї, ґ
                _ => ch,
            };

            let f = if self.frequency[i] == 0.0 {
                -10.0
            } else {
                self.frequency[i]
            };
            factor += f * scale[ch];
        }

        factor
    }
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

    // If not valid UTF-8 or no Cyrillic, try to decode from legacy encodings
    let try_encodings = ["Windows-1251", "CP866", "KOI8-R", "KOI8-U"];
    let mut frequencies = Vec::new();

    for &encoding in &try_encodings {
        let mut freq = CharFrequencies::new(encoding);

        // Try to decode from this encoding to UTF-8
        if let Ok(utf8_bytes) = convert_encoding(filename, encoding, "UTF-8") {
            if let Ok(utf8_str) = std::str::from_utf8(&utf8_bytes) {
                // Check if the result contains Cyrillic
                let has_cyrillic = utf8_str
                    .chars()
                    .any(|c| matches!(c, '\u{0400}'..='\u{04FF}' | '\u{0500}'..='\u{052F}'));

                if has_cyrillic {
                    // Convert to KOI8-U for frequency analysis
                    if let Ok(koi8u_bytes) = convert_encoding(&utf8_bytes, "UTF-8", "KOI8-U") {
                        for &ch in &koi8u_bytes {
                            freq.add_character(ch);
                        }
                    }
                }
            }
        }

        frequencies.push(freq);
    }

    // Sort by cyrillic factor (highest first) and character count
    frequencies.sort_by(|a, b| {
        if a.characters_seen > 0 && a.characters_seen < b.characters_seen {
            std::cmp::Ordering::Less
        } else if b.characters_seen > 0 && a.characters_seen > b.characters_seen {
            std::cmp::Ordering::Greater
        } else {
            let factor_a = a.cyrillic_factor();
            let factor_b = b.cyrillic_factor();
            factor_b
                .partial_cmp(&factor_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        }
    });

    if verbose >= 1 {
        println!("For filename detection:");
        for freq in &frequencies {
            println!(
                "\t{} factor {:.2} ({})",
                freq.encoding,
                freq.cyrillic_factor(),
                freq.characters_seen
            );
        }
    }

    // If no encoding produced good Cyrillic, default to UTF-8
    if frequencies.is_empty() || frequencies[0].characters_seen == 0 {
        "UTF-8"
    } else {
        frequencies[0].encoding
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
