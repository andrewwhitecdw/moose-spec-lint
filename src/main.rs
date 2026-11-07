//! Command-line entry point for `moose-spec-lint`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};

use moose_spec_lint::check::{self, Finding, Options, Severity};
use moose_spec_lint::hit;

#[derive(Parser)]
#[command(
    name = "moose-spec-lint",
    about = "Lint MOOSE test specification (tests) files",
    version
)]
struct Cli {
    /// Files or directories to lint; directories are searched recursively
    /// for files named `tests`
    #[arg(required = true)]
    paths: Vec<PathBuf>,

    /// Skip the requirement/design/issues SQA checks
    #[arg(long)]
    no_sqa: bool,

    /// Treat warnings as errors for the exit status
    #[arg(long)]
    strict: bool,

    /// Accept an additional tester type (repeatable); for application testers
    #[arg(long = "allow-tester", value_name = "NAME")]
    allow_tester: Vec<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,
}

#[derive(Clone, PartialEq, Eq, ValueEnum)]
enum Format {
    Human,
    Github,
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    let files = moose_spec_lint::collect_spec_files(&cli.paths).context("collecting spec files")?;

    let mut options = Options {
        no_sqa: cli.no_sqa,
        allowed_testers: cli.allow_tester.into_iter().collect(),
        ..Default::default()
    };

    let mut findings = Vec::new();
    for file in &files {
        options.spec_dir = file
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let src =
            std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        match hit::parse(&src) {
            Ok(root) => findings.extend(check::lint(file, &root, &options)),
            Err(err) => findings.push(Finding {
                path: file.clone(),
                line: err.line,
                severity: Severity::Error,
                message: format!("parse error: {}", err.message),
            }),
        }
    }

    for finding in &findings {
        println!("{}", render(finding, cli.format == Format::Github));
    }

    let errors = findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();
    let warnings = findings.len() - errors;
    if !findings.is_empty() {
        eprintln!(
            "{errors} errors, {warnings} warnings in {} spec file{}",
            files.len(),
            if files.len() == 1 { "" } else { "s" }
        );
    }

    if errors > 0 || (cli.strict && warnings > 0) {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn render(finding: &Finding, github: bool) -> String {
    let level = finding.severity.label();
    if github {
        // GitHub command annotations: %-escape CR/LF and the percent sign.
        let message = finding
            .message
            .replace('%', "%25")
            .replace('\r', "%0D")
            .replace('\n', "%0A");
        format!(
            "::{level} file={file},line={line}::{message}",
            file = finding.path.display(),
            line = finding.line,
        )
    } else {
        format!(
            "{file}:{line}: {level}: {message}",
            file = finding.path.display(),
            line = finding.line,
            message = finding.message,
        )
    }
}
