//! `manual`: builds Oxide's HTML manual from `docs/manual/`, or only checks it.
//!
//! ```text
//! cargo run -p manual                      # build to out/manual
//! cargo run -p manual -- --out DIR         # build to DIR
//! cargo run -p manual -- --check           # check every reference, write nothing
//!   --src DIR      the manual's folder (default docs/manual)
//!   --shots DIR    the screenshots (default <src>/shots)
//!   --strict       a missing screenshot is an error, not a warning
//!   --require-examples  an MCP action without an example is an error, not a warning
//!   --version V    the version the footer names (installer.ps1 passes its own)
//! cargo run -p manual -- --list-shots      # shots.toml as command lines (scripts/manual_shots.ps1)
//! cargo run -p manual -- --pack-shots DIR|PNG ...  # re-encode screenshots without loss, smaller
//! cargo run -p manual -- --readme-images LIST [--out DIR]  # the README's images (28.3)
//! ```
//!
//! Prints each problem (`FAIL: file:line: ...` for errors, `warning: ...`), then `RESULT:`
//! (with a build, the images' and the whole manual's sizes); exits 1 on any error. Relative
//! paths are taken from the repository's root, wherever it is run from.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use manual::check::Severity;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            println!("FAIL: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<ExitCode> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let from_root = |p: String| {
        let p = PathBuf::from(p);
        if p.is_absolute() {
            p
        } else {
            root.join(p)
        }
    };
    let mut out = None;
    let mut src = None;
    let mut shots = None;
    let mut check_only = false;
    let mut strict = false;
    let mut require_examples = false;
    let mut list_shots = false;
    let mut pack_shots: Vec<PathBuf> = Vec::new();
    let mut readme_images = None;
    let mut version = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| anyhow::anyhow!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--out" => out = Some(from_root(value()?)),
            "--src" => src = Some(from_root(value()?)),
            "--shots" => shots = Some(from_root(value()?)),
            "--check" => check_only = true,
            "--strict" => strict = true,
            "--require-examples" => require_examples = true,
            "--list-shots" => list_shots = true,
            "--pack-shots" => pack_shots.push(from_root(value()?)),
            "--readme-images" => readme_images = Some(from_root(value()?)),
            "--version" => version = Some(value()?),
            "-h" | "--help" => {
                println!(
                    "manual [--out DIR | --check | --list-shots] [--src DIR] [--shots DIR] [--version V] \
                     [--strict] [--require-examples]\n\
                     Builds Oxide's manual from docs/manual to out/manual (or only checks it,\n\
                     or prints shots.toml's shots as tab-separated command lines).\n\
                     manual --readme-images LIST [--out DIR] [--shots DIR]\n\
                     Makes the README's images in LIST (docs/images/images.toml) from the\n\
                     manual's shots into DIR (default: LIST's folder)."
                );
                return Ok(ExitCode::SUCCESS);
            }
            other => anyhow::bail!("unknown argument {other} (see --help)"),
        }
    }
    let src = src.unwrap_or_else(|| root.join("docs/manual"));
    if let Some(list) = readme_images {
        // Before reading the manual's pages: the README's images need only the shots.
        let shots = shots.unwrap_or_else(|| src.join("shots"));
        let out = out.unwrap_or_else(|| list.parent().unwrap_or(&root).to_path_buf());
        return make_readme_images(&list, &shots, &out);
    }
    if !pack_shots.is_empty() {
        // Each value a folder (its PNGs) or one PNG.
        let mut files = Vec::new();
        for path in &pack_shots {
            if path.is_dir() {
                let entries = std::fs::read_dir(path)
                    .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
                for entry in entries {
                    let file = entry?.path();
                    if file.extension().is_some_and(|e| e == "png") {
                        files.push(file);
                    }
                }
            } else {
                files.push(path.clone());
            }
        }
        let (mut before, mut after) = (0, 0);
        for file in &files {
            let (b, a) = manual::shots::pack(file)?;
            before += b;
            after += a;
        }
        println!(
            "RESULT: PASS packed {} screenshot(s): {:.2} MB to {:.2} MB, no pixel changed",
            files.len(),
            before as f64 / (1024.0 * 1024.0),
            after as f64 / (1024.0 * 1024.0)
        );
        return Ok(ExitCode::SUCCESS);
    }
    if list_shots {
        let path = src.join(manual::shots::LIST);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        print!("{}", manual::shots::ShotList::read(&text)?.command_lines());
        return Ok(ExitCode::SUCCESS);
    }
    let shots = shots.unwrap_or_else(|| src.join("shots"));
    let source = manual::Source::read(&src, &shots)?;
    let options = manual::Options {
        strict_shots: strict,
        version: version.unwrap_or_else(|| manual::VERSION.to_string()),
        require_examples,
    };
    let built = manual::build(&source, &options, &manual::mcp::Reference::load());

    for problem in &built.problems {
        match problem.severity {
            Severity::Error => println!("FAIL: {problem}"),
            Severity::Warning => println!("warning: {problem}"),
        }
    }
    let errors = built.errors();
    let warnings = built.problems.len() - errors;
    if errors > 0 {
        println!("RESULT: FAIL the manual has {errors} broken reference(s) or error(s)");
        return Ok(ExitCode::FAILURE);
    }
    let summary = format!(
        "{} pages, {} searchable sections, {warnings} warning(s), documents Oxide {}",
        built.pages.len(),
        built.index.entries.len(),
        options.version
    );
    if check_only {
        println!("RESULT: PASS manual checked: {summary}");
        return Ok(ExitCode::SUCCESS);
    }
    let out = out.unwrap_or_else(|| root.join("out/manual"));
    let written = manual::write(&built, &out)?;
    for (path, bytes, encoding) in &written.images {
        let how = match encoding {
            manual::shots::Encoding::Exact => "palette, exact".to_string(),
            manual::shots::Encoding::Quantised(psnr) => format!("palette, {psnr:.1} dB"),
            manual::shots::Encoding::FullColour => "full colour".to_string(),
        };
        println!("image {path}: {:.1} KB ({how})", *bytes as f64 / 1024.0);
    }
    let images = written.images.len();
    let image_kb = written.image_bytes() as f64 / 1024.0;
    let out = out.canonicalize().unwrap_or(out);
    println!(
        "RESULT: PASS manual built to {}: {summary}; {images} image(s), {image_kb:.0} KB ({:.1} KB \
         each on average), {:.2} MB in all",
        out.display().to_string().trim_start_matches(r"\\?\"),
        if images > 0 { image_kb / images as f64 } else { 0.0 },
        written.total as f64 / (1024.0 * 1024.0)
    );
    Ok(ExitCode::SUCCESS)
}

/// `--readme-images`: each image's line, files in `out` the list doesn't make (a warning: the
/// README may still use one), then `RESULT:`; fails past [`manual::readme_images::MAX_TOTAL`].
fn make_readme_images(list: &Path, shots: &Path, out: &Path) -> anyhow::Result<ExitCode> {
    use manual::readme_images::{make, ImageList, MAX_TOTAL};
    let text = std::fs::read_to_string(list)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", list.display()))?;
    let made = make(&ImageList::read(&text)?, shots, out)?;
    let mut total = 0;
    for image in &made {
        let how = match image.encoding {
            manual::shots::Encoding::Exact => "palette, exact".to_string(),
            manual::shots::Encoding::Quantised(psnr) => format!("palette, {psnr:.1} dB"),
            manual::shots::Encoding::FullColour => "full colour".to_string(),
        };
        println!(
            "image {}.png: {}x{}, {:.1} KB ({how}){}",
            image.name,
            image.size[0],
            image.size[1],
            image.bytes as f64 / 1024.0,
            if image.changed { "" } else { ", unchanged" }
        );
        total += image.bytes;
    }
    let names: Vec<String> = made.iter().map(|m| format!("{}.png", m.name)).collect();
    if let Ok(entries) = std::fs::read_dir(out) {
        let mut others: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|f| f.ends_with(".png") && !names.contains(f))
            .collect();
        others.sort();
        for other in others {
            println!(
                "warning: {other} in {} isn't in the list (delete it if the README no longer uses                  it)",
                out.display()
            );
        }
    }
    let kb = total as f64 / 1024.0;
    if total > MAX_TOTAL {
        println!(
            "RESULT: FAIL the README's {} image(s) weigh {kb:.0} KB, over {} KB",
            made.len(),
            MAX_TOTAL / 1024
        );
        return Ok(ExitCode::FAILURE);
    }
    println!(
        "RESULT: PASS {} README image(s), {kb:.0} KB in all (at most {} KB)",
        made.len(),
        MAX_TOTAL / 1024
    );
    Ok(ExitCode::SUCCESS)
}
