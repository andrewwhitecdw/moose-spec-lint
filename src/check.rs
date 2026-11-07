//! Lint rules for MOOSE test specification files.
//!
//! The rules are modelled on the MOOSE TestHarness parameter definitions in
//! `python/TestHarness/testers/` and the SQA requirements documented in the
//! MOOSE repository (every test carries `requirement`, `design`, and
//! `issues` keys).
//!
//! Section semantics match the TestHarness: a section with a `type`
//! parameter is a test; a section without one is a *group* whose parameters
//! inherit into every test below it (this is how groups share SQA keys).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::hit::{Entry, Node};

/// Severity of a lint finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// A violation of a rule the TestHarness or SQA process will not catch.
    Error,
    /// Suspicious but possibly intentional.
    Warning,
}

impl Severity {
    /// Renders as `error` or `warning`.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// A single lint finding anchored to a file and line.
#[derive(Debug, Clone)]
pub struct Finding {
    /// File the finding belongs to.
    pub path: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// Severity.
    pub severity: Severity,
    /// Human-readable description.
    pub message: String,
}

/// Options controlling which checks run.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// When true, skip the `requirement`/`design`/`issues` checks.
    pub no_sqa: bool,
    /// Additional tester type names to accept (application-defined testers).
    pub allowed_testers: HashSet<String>,
    /// Directory used to resolve relative file references from a spec.
    pub spec_dir: PathBuf,
}

/// A test section together with the group sections it inherits from,
/// ordered nearest-first after the test itself is considered.
struct TestCtx<'a> {
    node: &'a Node,
    groups: Vec<&'a Node>,
}

impl TestCtx<'_> {
    /// Full path from the root, e.g. `Tests/Group/test`, matching the
    /// unique identifier the TestHarness uses.
    fn full_path(&self) -> String {
        let mut parts: Vec<&str> = self.groups.iter().map(|g| g.name.as_str()).collect();
        parts.push(self.node.name.as_str());
        parts.join("/")
    }

    /// Effective value of a parameter: the test's own value wins, then the
    /// nearest enclosing group, and so on up the chain.
    fn effective(&self, key: &str) -> Option<&str> {
        self.node
            .param(key)
            .or_else(|| self.groups.iter().find_map(|g| g.param(key)))
            .map(|v| v.as_str())
    }
}

/// Lint one parsed spec tree, producing findings in a deterministic order.
pub fn lint(path: &Path, root: &Node, options: &Options) -> Vec<Finding> {
    let mut tests = Vec::new();
    let mut groups = Vec::new();
    collect_tests(root, &mut groups, &mut tests);
    drop(groups);

    let mut findings = Vec::new();
    let names: HashSet<&str> = tests
        .iter()
        .filter_map(|t| t.node.name.rsplit('/').next())
        .collect();
    for test in &tests {
        check_test(path, test, &names, options, &mut findings);
    }
    check_orphan_sections(path, root, &mut findings);
    check_duplicates(path, &tests, &mut findings);
    findings.sort_by(|a, b| (&a.path, a.line, &a.message).cmp(&(&b.path, b.line, &b.message)));
    findings
}

/// Gather test sections (those with `type`) plus their inheritance chain.
fn collect_tests<'a>(node: &'a Node, groups: &mut Vec<&'a Node>, out: &mut Vec<TestCtx<'a>>) {
    for child in node.sections() {
        if child.param("type").is_some() {
            out.push(TestCtx {
                node: child,
                groups: groups.clone(),
            });
        } else {
            groups.push(child);
            collect_tests(child, groups, out);
            groups.pop();
        }
    }
}

fn check_test(
    path: &Path,
    test: &TestCtx<'_>,
    file_test_names: &HashSet<&str>,
    options: &Options,
    findings: &mut Vec<Finding>,
) {
    let node = test.node;
    let line = node.line;
    let tester = node
        .param("type")
        .expect("collect_tests filters on type")
        .as_str()
        .to_string();

    if !known_tester(&tester) && !options.allowed_testers.contains(&tester) {
        finding(
            path,
            line,
            Severity::Error,
            format!("unknown tester type '{tester}' (use --allow-tester for application testers)"),
            findings,
        );
    }

    // Unknown keys on the test itself are checked against its full tester
    // chain; keys on enclosing groups only make sense as base Tester params.
    let allowed = allowed_keys(&tester);
    for entry in &node.entries {
        let Entry::Param { key, line, .. } = entry else {
            continue;
        };
        if !allowed.is_empty() && !allowed.contains(key.as_str()) {
            let hint = closest(key, &allowed);
            let msg = match hint {
                Some(h) => format!("unknown parameter '{key}' (did you mean '{h}'?)"),
                None => format!("unknown parameter '{key}'"),
            };
            finding(path, *line, Severity::Warning, msg, findings);
        }
    }
    let base: HashSet<&'static str> = TESTER_KEYS_BASE.iter().copied().collect();
    for group in &test.groups {
        for entry in &group.entries {
            let Entry::Param { key, line, .. } = entry else {
                continue;
            };
            if !base.contains(key.as_str()) {
                finding(
                    path,
                    *line,
                    Severity::Warning,
                    format!(
                        "group parameter '{key}' is not a base Tester parameter (check the child tests)"
                    ),
                    findings,
                );
            }
        }
    }

    // RunApp-derived testers normally need an input file or command, but
    // cli_args-only runs (e.g. --show-copyable-inputs) are a legitimate
    // corner, so this is a warning.
    if derives_from(&tester, "RunApp")
        && test.effective("input").is_none()
        && test.effective("command").is_none()
    {
        finding(
            path,
            line,
            Severity::Warning,
            format!("test of type '{tester}' has neither 'input' nor 'command'"),
            findings,
        );
    }
    for (tester_name, key) in [
        ("CSVDiff", "csvdiff"),
        ("Exodiff", "exodiff"),
        ("JSONDiff", "jsondiff"),
        ("XMLDiff", "xmldiff"),
        ("ImageDiff", "imagediff"),
    ] {
        if tester == tester_name && test.effective(key).is_none() {
            finding(
                path,
                line,
                Severity::Error,
                format!("test of type '{tester_name}' is missing required '{key}' parameter"),
                findings,
            );
        }
    }

    if !options.no_sqa {
        for sqa in ["requirement", "design", "issues"] {
            match test.effective(sqa) {
                None => finding(
                    path,
                    line,
                    Severity::Error,
                    format!(
                        "missing SQA parameter '{sqa}' (not set on the test or any enclosing group)"
                    ),
                    findings,
                ),
                Some(v) if v.trim().is_empty() => finding(
                    path,
                    line,
                    Severity::Error,
                    format!("SQA parameter '{sqa}' is empty"),
                    findings,
                ),
                Some(_) => {}
            }
        }
    }

    // Referenced input files must exist. `input` may hold a space-separated
    // list, and tests that never execute may use placeholder names.
    let executes = !matches!(
        test.effective("should_execute")
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("false")
    );
    if executes {
        if let Some(v) = test.effective("input") {
            // Tokens containing '=' are command-line overrides
            // (`input = 'a.i Variables/x=1'`), not files.
            for name in v.split_whitespace().filter(|t| !t.contains('=')) {
                if !options.spec_dir.join(name).is_file() {
                    finding(
                        path,
                        line,
                        Severity::Error,
                        format!("referenced 'input' file '{name}' does not exist"),
                        findings,
                    );
                }
            }
        }
    }
    // Golden-output files live in `gold/` by default (overridable with gold_dir).
    let gold_dir = test.effective("gold_dir").unwrap_or("gold");
    for key in [
        "csvdiff",
        "exodiff",
        "jsondiff",
        "xmldiff",
        "imagediff",
        "custom_cmp",
        "comparison_file",
        "schemadiff",
    ] {
        if let Some(v) = test.effective(key) {
            for name in v.split_whitespace() {
                let direct = options.spec_dir.join(name);
                let golded = options.spec_dir.join(gold_dir).join(name);
                if !direct.is_file() && !golded.is_file() {
                    finding(
                        path,
                        line,
                        Severity::Error,
                        format!(
                            "referenced '{key}' file '{name}' does not exist (checked . and {gold_dir}/)"
                        ),
                        findings,
                    );
                }
            }
        }
    }

    // prereq entries should name tests in the same file. A name containing
    // '/' is a path-qualified reference into another spec file.
    if let Some(v) = test.effective("prereq") {
        for name in v.split_whitespace() {
            if name.contains('/') {
                continue;
            }
            if !file_test_names.contains(name) {
                finding(
                    path,
                    line,
                    Severity::Warning,
                    format!("prereq '{name}' does not match a test in this file"),
                    findings,
                );
            }
        }
    }
}

/// Sections that carry parameters but have neither `type` nor children do
/// nothing in the TestHarness and are usually a forgotten `type =`.
fn check_orphan_sections(path: &Path, root: &Node, findings: &mut Vec<Finding>) {
    fn walk(path: &Path, node: &Node, findings: &mut Vec<Finding>) {
        for child in node.sections() {
            if child.param("type").is_none() && child.sections().next().is_none() {
                let has_params = child
                    .entries
                    .iter()
                    .any(|e| matches!(e, Entry::Param { .. }));
                if has_params {
                    finding(
                        path,
                        child.line,
                        Severity::Warning,
                        format!(
                            "section '{}' has parameters but no 'type' and no child sections",
                            child.name
                        ),
                        findings,
                    );
                }
            }
            walk(path, child, findings);
        }
    }
    walk(path, root, findings);
}

fn check_duplicates(path: &Path, tests: &[TestCtx<'_>], findings: &mut Vec<Finding>) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for test in tests {
        let full = test.full_path();
        if let Some(first) = seen.insert(full.clone(), test.node.line) {
            finding(
                path,
                test.node.line,
                Severity::Warning,
                format!("duplicate test path '{full}' (first defined on line {first})"),
                findings,
            );
        }
    }
}

fn finding(
    path: &Path,
    line: usize,
    severity: Severity,
    message: impl Into<String>,
    findings: &mut Vec<Finding>,
) {
    findings.push(Finding {
        path: path.to_path_buf(),
        line,
        severity,
        message: message.into(),
    });
}

/// (name, parent, own parameter keys) for every framework tester and the
/// base `Tester` (empty parent), transcribed from
/// `python/TestHarness/testers/*.py`.
static TESTER_KEYS: &[(&str, &str, &[&str])] = &[
    ("Tester", "", TESTER_KEYS_BASE),
    (
        "RunApp",
        "Tester",
        &[
            "input",
            "test_name",
            "input_switch",
            "errors",
            "expect_out",
            "match_literal",
            "absent_out",
            "executable_pattern",
            "delete_output_before_running",
            "custom_evaluation_script",
            "command",
            "command_proxy",
            "max_parallel",
            "min_parallel",
            "max_threads",
            "min_threads",
            "redirect_output",
            "allow_warnings",
            "allow_unused",
            "allow_deprecated",
            "no_error_deprecated",
            "no_additional_cli_args",
            "capture_perf_graph",
            "perf_graph_live",
            "valgrind",
            "compute_devices",
        ],
    ),
    ("RunCommand", "Tester", &["command", "test_name"]),
    ("FileTester", "RunApp", &["gold_dir", "abs_zero", "rel_err"]),
    (
        "Exodiff",
        "FileTester",
        &[
            "exodiff",
            "exodiff_opts",
            "custom_cmp",
            "use_old_floor",
            "map",
            "partial",
        ],
    ),
    (
        "CSVDiff",
        "FileTester",
        &[
            "csvdiff",
            "override_columns",
            "override_rel_err",
            "override_abs_zero",
            "comparison_file",
            "ignore_columns",
            "custom_columns",
        ],
    ),
    (
        "CSVValidationTester",
        "FileTester",
        &["csvdiff", "err_type", "mean_limit", "std_limit"],
    ),
    (
        "CheckFiles",
        "FileTester",
        &["check_files", "check_not_exists", "file_expect_out"],
    ),
    (
        "ImageDiff",
        "FileTester",
        &["imagediff", "allowed", "allowed_linux", "allowed_darwin"],
    ),
    (
        "SchemaDiff",
        "RunApp",
        &[
            "schemadiff",
            "gold_dir",
            "gold_file",
            "ignored_items",
            "rel_err",
            "abs_zero",
        ],
    ),
    (
        "JSONDiff",
        "SchemaDiff",
        &[
            "jsondiff",
            "skip_keys",
            "ignored_regex_items",
            "keep_system_information",
            "keep_reporter_types",
        ],
    ),
    ("XMLDiff", "SchemaDiff", &["xmldiff", "ignored_attributes"]),
    (
        "PythonUnitTest",
        "RunApp",
        &["input", "test_case", "buffer", "separate"],
    ),
    ("MMSTest", "PythonUnitTest", &[]),
    (
        "PetscJacobianTester",
        "RunApp",
        &[
            "input",
            "ratio_tol",
            "difference_tol",
            "state",
            "run_sim",
            "turn_off_exodus_output",
            "only_final_jacobian",
        ],
    ),
    (
        "RunException",
        "RunApp",
        &["expect_err", "expect_assert", "expect_exit_code"],
    ),
    (
        "AnalyzeJacobian",
        "FileTester",
        &[
            "input",
            "test_name",
            "expect_out",
            "resize_mesh",
            "off_diagonal",
            "mesh_size",
        ],
    ),
    ("SignalTester", "RunApp", &["signal", "signal_ready"]),
    (
        "TaoGradientTester",
        "RunApp",
        &["max_rel_tol", "tao_fd_delta"],
    ),
];

const TESTER_KEYS_BASE: &[&str] = &[
    "type",
    "max_time",
    "skip",
    "deleted",
    "unique_test_id",
    "heavy",
    "group",
    "prereq",
    "skip_checks",
    "scale_refine",
    "success_message",
    "cli_args",
    "use_shell",
    "allow_test_objects",
    "valgrind",
    "max_buffer_size",
    "parallel_scheduling",
    "min_slots",
    "mesh_mode",
    "recover",
    "restep",
    "expect_exit_code",
    "capabilities",
    "dynamic_capabilities",
    "depend_files",
    "env_vars",
    "env_vars_not_set",
    "should_execute",
    "required_submodule",
    "check_input",
    "display_required",
    "timing",
    "python",
    "required_python_packages",
    "requires",
    "working_directory",
    "requirement",
    "design",
    "issues",
    "detail",
    "validation",
    "verification",
    "deprecated",
    "collections",
    "classification",
    "hpc_mem_per_cpu",
    "validation_test",
];

fn tester_entry(
    name: &str,
) -> Option<&'static (&'static str, &'static str, &'static [&'static str])> {
    TESTER_KEYS.iter().find(|(n, _, _)| *n == name)
}

fn known_tester(name: &str) -> bool {
    tester_entry(name).is_some()
}

/// True when `tester` is `ancestor` or derives from it.
fn derives_from(tester: &str, ancestor: &str) -> bool {
    let mut current = tester;
    while let Some(entry) = tester_entry(current) {
        if current == ancestor {
            return true;
        }
        if entry.1.is_empty() {
            break;
        }
        current = entry.1;
    }
    false
}

/// Parameter keys valid for a tester, resolved through its inheritance
/// chain. Returns an empty set for unknown testers so no key warnings pile
/// up on top of the unknown-tester error.
fn allowed_keys(tester: &str) -> HashSet<&'static str> {
    let mut out = HashSet::new();
    let mut current = tester;
    while let Some(entry) = tester_entry(current) {
        out.extend(entry.2.iter().copied());
        if entry.1.is_empty() {
            break;
        }
        current = entry.1;
    }
    out
}

/// Levenshtein distance over the tiny key lists involved here.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn closest(key: &str, allowed: &HashSet<&'static str>) -> Option<&'static str> {
    allowed
        .iter()
        .map(|k| (levenshtein(key, k), *k))
        .filter(|(d, _)| *d <= 3)
        .min()
        .map(|(_, k)| k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(dir: &Path) -> Options {
        Options {
            spec_dir: dir.to_path_buf(),
            ..Default::default()
        }
    }

    fn messages(findings: &[Finding]) -> Vec<String> {
        findings.iter().map(|f| f.message.clone()).collect()
    }

    #[test]
    fn flags_typo_with_did_you_mean() {
        let root = crate::hit::parse(
            "[Tests]\n  [./t]\n    type = RunApp\n    requirment = 'x'\n    input = 't.i'\n  []\n[]",
        )
        .expect("parses");
        let findings = lint(Path::new("tests"), &root, &opts(Path::new("")));
        let msgs = messages(&findings);
        assert!(
            msgs.iter()
                .any(|m| m.contains("did you mean 'requirement'")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("referenced 'input' file 't.i' does not exist"))
        );
    }

    #[test]
    fn group_sqa_keys_inherit_into_tests() {
        let root = crate::hit::parse(
            "[Tests]\n  [./group]\n    requirement = 'shared'\n    design = 'g.md'\n    issues = '#1'\n    [./child]\n      type = RunApp\n      input = 'c.i'\n    []\n  []\n[]",
        )
        .expect("parses");
        let findings = lint(Path::new("tests"), &root, &opts(Path::new("")));
        let sqa: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("SQA"))
            .collect();
        assert!(sqa.is_empty(), "{findings:?}");
    }

    #[test]
    fn group_with_runapp_key_warns() {
        let root = crate::hit::parse(
            "[Tests]\n  [./group]\n    input = 'g.i'\n    [./child]\n      type = RunApp\n      input = 'c.i'\n    []\n  []\n[]",
        )
        .expect("parses");
        let findings = lint(Path::new("tests"), &root, &opts(Path::new("")));
        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("group parameter 'input'")),
            "{findings:?}"
        );
    }

    #[test]
    fn childless_parameterized_section_warns() {
        let root =
            crate::hit::parse("[Tests]\n  [./oops]\n    input = 'a.i'\n  []\n[]").expect("parses");
        let findings = lint(Path::new("tests"), &root, &opts(Path::new("")));
        assert!(
            findings
                .iter()
                .any(|f| f.severity == Severity::Warning && f.message.contains("no 'type'")),
            "{findings:?}"
        );
    }

    #[test]
    fn unknown_tester_is_an_error() {
        let root = crate::hit::parse("[Tests]\n  [./t]\n    type = FancyAppTester\n  []\n[]")
            .expect("parses");
        let findings = lint(Path::new("tests"), &root, &opts(Path::new("")));
        assert!(findings.iter().any(|f| f.severity == Severity::Error
            && f.message.contains("unknown tester type 'FancyAppTester'")));
    }

    #[test]
    fn allowed_tester_suppresses_unknown_error() {
        let mut options = opts(Path::new(""));
        options.allowed_testers.insert("FancyAppTester".to_string());
        let root = crate::hit::parse(
            "[Tests]\n  [./t]\n    type = FancyAppTester\n    command = 'true'\n    issues = '#1'\n    design = 'd.md'\n    requirement = 'runs'\n  []\n[]",
        )
        .expect("parses");
        let findings = lint(Path::new("tests"), &root, &options);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn sqa_keys_enforced_unless_disabled() {
        let src = "[Tests]\n  [./t]\n    type = RunApp\n    input = 't.i'\n  []\n[]";
        let root = crate::hit::parse(src).expect("parses");
        let findings = lint(Path::new("tests"), &root, &opts(Path::new("")));
        let sqa: Vec<_> = findings
            .iter()
            .filter(|f| f.message.contains("SQA"))
            .collect();
        assert_eq!(sqa.len(), 3, "{findings:?}");

        let no_sqa = Options {
            no_sqa: true,
            ..opts(Path::new(""))
        };
        let findings = lint(Path::new("tests"), &root, &no_sqa);
        assert!(findings.iter().all(|f| !f.message.contains("SQA")));
    }

    #[test]
    fn diff_tester_requires_golden_parameter() {
        let root =
            crate::hit::parse("[Tests]\n  [./t]\n    type = CSVDiff\n    input = 'a.i'\n  []\n[]")
                .expect("parses");
        let findings = lint(Path::new("tests"), &root, &opts(Path::new("")));
        assert!(
            findings
                .iter()
                .any(|f| f.message.contains("missing required 'csvdiff' parameter")),
            "{findings:?}"
        );
    }

    #[test]
    fn should_execute_false_skips_input_check() {
        let root = crate::hit::parse(
            "[Tests]\n  [./t]\n    type = CheckFiles\n    input = 'DUMMY'\n    should_execute = false\n    issues = '#1'\n    design = 'd.md'\n    requirement = 'runs'\n  []\n[]",
        )
        .expect("parses");
        let findings = lint(Path::new("tests"), &root, &opts(Path::new("")));
        assert!(
            !findings
                .iter()
                .any(|f| f.message.contains("does not exist")),
            "{findings:?}"
        );
    }

    #[test]
    fn multi_input_lists_checked_per_file() {
        let dir = std::env::temp_dir().join("msl-test-multi-input");
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("a.i"), "[Mesh]\n[]\n").expect("a.i");
        let src = "[Tests]\n  [./t]\n    type = RunApp\n    input = 'a.i missing.i'\n    issues = '#1'\n    design = 'd.md'\n    requirement = 'runs'\n  []\n[]";
        let root = crate::hit::parse(src).expect("parses");
        let findings = lint(Path::new("tests"), &root, &opts(&dir));
        assert!(
            findings.iter().any(|f| f
                .message
                .contains("referenced 'input' file 'missing.i' does not exist")),
            "{findings:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn happy_path_is_clean() {
        let dir = std::env::temp_dir().join("msl-test-happy");
        std::fs::create_dir_all(dir.join("gold")).expect("gold dir");
        std::fs::write(dir.join("a.i"), "[Mesh]\n[]\n").expect("write input");
        std::fs::write(dir.join("gold").join("a_out.csv"), "time,v\n0,1\n").expect("write gold");
        let src = "[Tests]\n  [./t]\n    type = CSVDiff\n    input = 'a.i'\n    csvdiff = 'a_out.csv'\n    issues = '#1'\n    design = 'x.md'\n    requirement = 'runs'\n  []\n[]";
        let root = crate::hit::parse(src).expect("parses");
        let findings = lint(Path::new("tests"), &root, &opts(&dir));
        assert!(findings.is_empty(), "unexpected findings: {findings:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prereq_and_duplicates_warn() {
        let src = "[Tests]\n  [./a]\n    type = RunApp\n    input = 'a.i'\n    prereq = 'b other/file:b'\n  [../a]\n    type = RunApp\n    input = 'a.i'\n  []\n[]";
        let root = crate::hit::parse(src).expect("parses");
        let findings = lint(Path::new("tests"), &root, &opts(Path::new("")));
        let msgs = messages(&findings);
        assert!(msgs.iter().any(|m| m.contains("prereq 'b'")), "{msgs:?}");
        assert!(
            !msgs.iter().any(|m| m.contains("'other/file:b'")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter().any(|m| m.contains("duplicate test path")),
            "{msgs:?}"
        );
    }
}
