//! End-to-end tests for the `moose-spec-lint` binary.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("msl-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("gold")).expect("fixture dirs");
    fs::write(dir.join("a.i"), "[Mesh]\n[]\n").expect("input file");
    fs::write(dir.join("gold").join("a_out.csv"), "time,v\n0,1\n").expect("gold file");
    fs::write(
        dir.join("tests"),
        "[Tests]\n  [./good]\n    type = CSVDiff\n    input = 'a.i'\n    csvdiff = 'a_out.csv'\n    issues = '#1'\n    design = 'd.md'\n    requirement = 'runs'\n  []\n  [./bad]\n    type = RunApp\n    input = 'missing.i'\n    requirment = 'typo'\n  []\n[]\n",
    )
    .expect("spec file");
    dir
}

#[test]
fn lints_directory_and_reports_errors() {
    let dir = fixture("lints");
    let output = Command::new(env!("CARGO_BIN_EXE_moose-spec-lint"))
        .arg(&dir)
        .output()
        .expect("run binary");
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("error"), "{stdout}");
    assert!(stdout.contains("missing.i"), "{stdout}");
    assert!(stdout.contains("did you mean 'requirement'"), "{stdout}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn clean_tree_exits_zero() {
    let dir = fixture("clean");
    fs::write(
        dir.join("tests"),
        "[Tests]\n  [./good]\n    type = CSVDiff\n    input = 'a.i'\n    csvdiff = 'a_out.csv'\n    issues = '#1'\n    design = 'd.md'\n    requirement = 'runs'\n  []\n[]\n",
    )
    .expect("clean spec");
    let output = Command::new(env!("CARGO_BIN_EXE_moose-spec-lint"))
        .arg(&dir)
        .output()
        .expect("run binary");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(!stdout.contains("error"), "{stdout}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn github_format_annotations() {
    let dir = fixture("github");
    let output = Command::new(env!("CARGO_BIN_EXE_moose-spec-lint"))
        .arg(&dir)
        .arg("--format")
        .arg("github")
        .output()
        .expect("run binary");
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(stdout.contains("::error file="), "{stdout}");
    fs::remove_dir_all(&dir).ok();
}
