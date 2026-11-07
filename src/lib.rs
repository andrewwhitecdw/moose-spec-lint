//! Lint MOOSE test specification (`tests`) files for missing SQA metadata,
//! misspelled parameters, dangling file references, and structural mistakes.
//!
//! The library is organised in two layers: [`hit`] parses the HIT syntax
//! used by spec files, and [`check`] applies the lint rules against the
//! parsed tree. The binary in `src/main.rs` is a thin wrapper.

pub mod check;
pub mod hit;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Collect spec files from the supplied paths. Directories are searched
/// recursively for files named exactly `tests`; files are used directly.
/// Results are sorted and deduplicated for deterministic output.
pub fn collect_spec_files(paths: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    for path in paths {
        if path.is_dir() {
            collect_in_dir(path, &mut out)?;
        } else {
            out.push(path.clone());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn collect_in_dir(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            // Never descend into VCS metadata or golden-output dirs.
            if name != ".git" && name != "gold" && !name.to_string_lossy().starts_with('.') {
                collect_in_dir(&path, out)?;
            }
        } else if name == "tests" {
            out.push(path);
        }
    }
    Ok(())
}
