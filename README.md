# zip-fix-filename-encoding

## About

Convert filenames inside ZIP archives from autodetected older Russian encodings (koi8-r, koi8-u, cp866, windows-1251) to UTF-8.

This tool does not touch the file contents, it just renames the files inside a ZIP archive.

**NOTE**: This is an LLM-aided Rust port of the [original C codebase](https://github.com/vlm/zip-fix-filename-encoding) for my personal use.

## Build and Install

    cargo build --release
    cargo install --path .

Or simply run directly:

    cargo run --release -- [OPTIONS] <filename.zip>...

## Usage

    Usage: runzip [OPTIONS] [FILES]...

    Arguments:
    [FILES]...
            ZIP files to process

    Options:
    -n, --dry-run
            Dry run. Do not modify the <file.zip>

    -v, --verbose...
            Verbose output (can be repeated)

    -s, --source <SOURCE_ENCODING>
            Set source encoding. Auto-detect, if not set

    -t, --target <TARGET_ENCODING>
            Set target encoding. Default is UTF-8
            
            [default: utf-8]

    -w, --windows
            Make archive readable on Windows (reverse operation) NOTE: -w implies -t cp866 (Yes! MS-DOS!)

    -h, --help
            Print help (see a summary with '-h')

    -V, --version
            Print version
