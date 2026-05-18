use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";
const UNCHANGED_CONTEXT_LINES: usize = 3;

#[derive(Debug, PartialEq, Eq)]
enum DiffLine<'a> {
    Equal(&'a str),
    Removed(&'a str),
    Added(&'a str),
}

#[derive(Debug)]
enum AppError {
    Help(String),
    Usage(String),
    GitDiffFailed {
        repo: PathBuf,
        range: String,
        status: String,
        stderr: String,
    },
    GitDiffIo {
        repo: PathBuf,
        range: String,
        source: io::Error,
    },
    Output(io::Error),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Help(message) => write!(f, "{message}"),
            AppError::Usage(message) => write!(f, "{message}"),
            AppError::GitDiffFailed {
                repo,
                range,
                status,
                stderr,
            } => {
                writeln!(
                    f,
                    "git diff failed in `{}` for range `{range}` with status {status}",
                    repo.display()
                )?;
                write!(f, "{}", stderr.trim_end())
            }
            AppError::GitDiffIo {
                repo,
                range,
                source,
            } => {
                write!(
                    f,
                    "failed to run git diff in `{}` for range `{range}`: {source}",
                    repo.display()
                )
            }
            AppError::Output(source) => write!(f, "failed to write output: {source}"),
        }
    }
}

fn main() -> ExitCode {
    match run(env::args_os()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(AppError::Help(message)) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(AppError::Usage(message)) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: impl IntoIterator<Item = OsString>) -> Result<(), AppError> {
    let options = Options::parse(args)?;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    render_repository(
        Path::new("."),
        Some(&options.before_range),
        Some(&options.after_range),
        &mut handle,
    )
}

struct Options {
    before_range: String,
    after_range: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SubmodulePatchSet {
    path: String,
    before_range: Option<String>,
    after_range: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct SubmoduleCommitRange {
    path: String,
    old_commit: String,
    new_commit: String,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, AppError> {
        let mut args = args.into_iter();
        let program = args
            .next()
            .and_then(|arg| arg.into_string().ok())
            .unwrap_or_else(|| "git-range-diff-better".to_string());

        let remaining = args
            .map(|arg| {
                arg.into_string().map_err(|_| {
                    AppError::Usage(format!(
                        "arguments must be valid UTF-8\n\n{}",
                        usage(&program)
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        if remaining.iter().any(|arg| arg == "-h" || arg == "--help") {
            return Err(AppError::Help(usage(&program)));
        }

        match remaining.as_slice() {
            [before_range, after_range] => Ok(Self {
                before_range: before_range.clone(),
                after_range: after_range.clone(),
            }),
            _ => Err(AppError::Usage(usage(&program))),
        }
    }
}

fn usage(program: &str) -> String {
    format!(
        "Usage: {program} <before-push-range> <after-push-range>\n\n\
         Example:\n  {program} abc123..def456 fed789..012345\n\n\
         Each range is passed directly to \
         `git diff --no-color --no-ext-diff --submodule=short <range>`."
    )
}

fn git_diff(repo: &Path, range: &str) -> Result<String, AppError> {
    let output = Command::new("git")
        .current_dir(repo)
        .args([
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--submodule=short",
            range,
        ])
        .output()
        .map_err(|source| AppError::GitDiffIo {
            repo: repo.to_path_buf(),
            range: range.to_string(),
            source,
        })?;

    if !output.status.success() {
        return Err(AppError::GitDiffFailed {
            repo: repo.to_path_buf(),
            range: range.to_string(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn render_repository(
    repo: &Path,
    before_range: Option<&str>,
    after_range: Option<&str>,
    writer: &mut impl Write,
) -> Result<(), AppError> {
    let before = optional_git_diff(repo, before_range)?;
    let after = optional_git_diff(repo, after_range)?;
    let diff = diff_normalized_lines(&before, &after);
    let changed_sections = changed_file_sections(&diff);

    write_compact_colored_diff_refs(&changed_sections, writer).map_err(AppError::Output)?;

    for submodule in submodule_patch_sets(&before, &after) {
        writeln!(writer, "\nSubmodule {}", submodule.path).map_err(AppError::Output)?;

        let submodule_repo = repo.join(&submodule.path);
        render_repository(
            &submodule_repo,
            submodule.before_range.as_deref(),
            submodule.after_range.as_deref(),
            writer,
        )?;
    }

    Ok(())
}

fn optional_git_diff(repo: &Path, range: Option<&str>) -> Result<String, AppError> {
    range.map_or_else(|| Ok(String::new()), |range| git_diff(repo, range))
}

fn submodule_patch_sets(before: &str, after: &str) -> Vec<SubmodulePatchSet> {
    let mut patch_sets = BTreeMap::<String, SubmodulePatchSet>::new();

    for range in submodule_commit_ranges(before) {
        let patch_set = patch_sets
            .entry(range.path.clone())
            .or_insert_with(|| SubmodulePatchSet {
                path: range.path.clone(),
                ..SubmodulePatchSet::default()
            });
        patch_set.before_range = Some(format!("{}..{}", range.old_commit, range.new_commit));
    }

    for range in submodule_commit_ranges(after) {
        let patch_set = patch_sets
            .entry(range.path.clone())
            .or_insert_with(|| SubmodulePatchSet {
                path: range.path.clone(),
                ..SubmodulePatchSet::default()
            });
        patch_set.after_range = Some(format!("{}..{}", range.old_commit, range.new_commit));
    }

    patch_sets.into_values().collect()
}

fn submodule_commit_ranges(diff: &str) -> Vec<SubmoduleCommitRange> {
    let mut ranges = Vec::new();
    let mut section = SubmoduleSection::default();

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            push_submodule_section(&mut ranges, &section);
            section = SubmoduleSection {
                path: parse_diff_git_path(line),
                ..SubmoduleSection::default()
            };
            continue;
        }

        if line == "new file mode 160000"
            || line == "deleted file mode 160000"
            || line.ends_with(" 160000")
        {
            section.is_submodule = true;
        }

        if let Some(commit) = line.strip_prefix("-Subproject commit ") {
            section.old_commit = commit.split_whitespace().next().map(str::to_string);
        } else if let Some(commit) = line.strip_prefix("+Subproject commit ") {
            section.new_commit = commit.split_whitespace().next().map(str::to_string);
        }
    }

    push_submodule_section(&mut ranges, &section);
    ranges
}

#[derive(Default)]
struct SubmoduleSection {
    path: Option<String>,
    is_submodule: bool,
    old_commit: Option<String>,
    new_commit: Option<String>,
}

fn push_submodule_section(output: &mut Vec<SubmoduleCommitRange>, section: &SubmoduleSection) {
    if !section.is_submodule {
        return;
    }

    let (Some(path), Some(old_commit), Some(new_commit)) = (
        section.path.as_ref(),
        section.old_commit.as_ref(),
        section.new_commit.as_ref(),
    ) else {
        return;
    };

    output.push(SubmoduleCommitRange {
        path: path.clone(),
        old_commit: old_commit.clone(),
        new_commit: new_commit.clone(),
    });
}

fn parse_diff_git_path(line: &str) -> Option<String> {
    let (_, path) = line.rsplit_once(" b/")?;
    Some(path.to_string())
}

#[cfg(test)]
fn diff_lines<'a>(before: &'a str, after: &'a str) -> Vec<DiffLine<'a>> {
    diff_normalized_lines_by(before, after, |line| line)
}

fn diff_normalized_lines<'a>(before: &'a str, after: &'a str) -> Vec<DiffLine<'a>> {
    diff_normalized_lines_by(before, after, normalize_git_diff_line)
}

fn diff_normalized_lines_by<'a>(
    before: &'a str,
    after: &'a str,
    normalize: impl Fn(&'a str) -> &'a str,
) -> Vec<DiffLine<'a>> {
    let before_lines = before.lines().collect::<Vec<_>>();
    let after_lines = after.lines().collect::<Vec<_>>();
    let before_keys = before_lines
        .iter()
        .map(|line| normalize(line))
        .collect::<Vec<_>>();
    let after_keys = after_lines
        .iter()
        .map(|line| normalize(line))
        .collect::<Vec<_>>();
    let mut lengths = vec![vec![0usize; after_lines.len() + 1]; before_lines.len() + 1];

    for before_index in (0..before_lines.len()).rev() {
        for after_index in (0..after_lines.len()).rev() {
            lengths[before_index][after_index] = if before_keys[before_index]
                == after_keys[after_index]
            {
                lengths[before_index + 1][after_index + 1] + 1
            } else {
                lengths[before_index + 1][after_index].max(lengths[before_index][after_index + 1])
            };
        }
    }

    let mut result = Vec::new();
    let mut before_index = 0;
    let mut after_index = 0;

    while before_index < before_lines.len() && after_index < after_lines.len() {
        if before_keys[before_index] == after_keys[after_index] {
            result.push(DiffLine::Equal(before_lines[before_index]));
            before_index += 1;
            after_index += 1;
        } else if lengths[before_index + 1][after_index] >= lengths[before_index][after_index + 1] {
            result.push(DiffLine::Removed(before_lines[before_index]));
            before_index += 1;
        } else {
            result.push(DiffLine::Added(after_lines[after_index]));
            after_index += 1;
        }
    }

    result.extend(
        before_lines[before_index..]
            .iter()
            .map(|line| DiffLine::Removed(line)),
    );
    result.extend(
        after_lines[after_index..]
            .iter()
            .map(|line| DiffLine::Added(line)),
    );
    result
}

fn normalize_git_diff_line(line: &str) -> &str {
    if line.starts_with("@@") {
        "@@"
    } else if let Some(mode) = normalized_git_index_line(line) {
        mode
    } else {
        line
    }
}

fn normalized_git_index_line(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("index ")?;
    let mut parts = rest.split_whitespace();
    let hashes = parts.next()?;

    if !hashes.contains("..") {
        return None;
    }

    Some(parts.next().unwrap_or("index"))
}

fn changed_file_sections<'a>(diff: &'a [DiffLine<'a>]) -> Vec<&'a DiffLine<'a>> {
    let mut changed = Vec::new();
    let mut section_start = 0;
    let mut index = 0;

    while index < diff.len() {
        if index > section_start && is_file_section_start(&diff[index]) {
            push_section_if_changed(diff, section_start, index, &mut changed);
            section_start = index;
        }

        index += 1;
    }

    push_section_if_changed(diff, section_start, diff.len(), &mut changed);
    changed
}

fn is_file_section_start(line: &DiffLine<'_>) -> bool {
    match line {
        DiffLine::Equal(line) | DiffLine::Removed(line) | DiffLine::Added(line) => {
            line.starts_with("diff --git ")
        }
    }
}

fn push_section_if_changed<'a>(
    diff: &'a [DiffLine<'a>],
    start: usize,
    end: usize,
    output: &mut Vec<&'a DiffLine<'a>>,
) {
    if diff[start..end]
        .iter()
        .any(|line| !matches!(line, DiffLine::Equal(_)))
    {
        output.extend(diff[start..end].iter());
    }
}

#[cfg(test)]
fn write_colored_diff(diff: &[DiffLine<'_>], writer: &mut impl Write) -> io::Result<()> {
    for line in diff {
        write_colored_line(line, writer)?;
    }

    Ok(())
}

#[cfg(test)]
fn write_colored_diff_refs(diff: &[&DiffLine<'_>], writer: &mut impl Write) -> io::Result<()> {
    for line in diff {
        write_colored_line(line, writer)?;
    }

    Ok(())
}

fn write_compact_colored_diff_refs(
    diff: &[&DiffLine<'_>],
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut index = 0;

    while index < diff.len() {
        if matches!(diff[index], DiffLine::Equal(_)) {
            let run_start = index;

            while index < diff.len() && matches!(diff[index], DiffLine::Equal(_)) {
                index += 1;
            }

            write_equal_run_compact(&diff[run_start..index], writer)?;
        } else {
            write_colored_line(diff[index], writer)?;
            index += 1;
        }
    }

    Ok(())
}

fn write_equal_run_compact(run: &[&DiffLine<'_>], writer: &mut impl Write) -> io::Result<()> {
    let max_visible_lines = UNCHANGED_CONTEXT_LINES * 2;

    if run.len() <= max_visible_lines + 1 {
        for line in run {
            write_colored_line(line, writer)?;
        }

        return Ok(());
    }

    let header_index = run.iter().rposition(|line| is_file_section_start(line));
    let tail_start = run.len() - UNCHANGED_CONTEXT_LINES;

    for line in &run[..UNCHANGED_CONTEXT_LINES] {
        write_colored_line(line, writer)?;
    }

    if let Some(header_index) = header_index {
        if (UNCHANGED_CONTEXT_LINES..tail_start).contains(&header_index) {
            write_colored_line(run[header_index], writer)?;
        }
    }

    for line in &run[tail_start..] {
        write_colored_line(line, writer)?;
    }

    Ok(())
}

fn write_colored_line(line: &DiffLine<'_>, writer: &mut impl Write) -> io::Result<()> {
    match line {
        DiffLine::Equal(line) => writeln!(writer, "  {line}"),
        DiffLine::Removed(line) => writeln!(writer, "{RED}- {line}{RESET}"),
        DiffLine::Added(line) => writeln!(writer, "{GREEN}+ {line}{RESET}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_lines_from_two_texts() {
        let before = "same\nold\nkeep\n";
        let after = "same\nnew\nkeep\n";

        assert_eq!(
            diff_lines(before, after),
            vec![
                DiffLine::Equal("same"),
                DiffLine::Removed("old"),
                DiffLine::Added("new"),
                DiffLine::Equal("keep"),
            ]
        );
    }

    #[test]
    fn colorizes_added_and_removed_lines() {
        let diff = vec![
            DiffLine::Equal("same"),
            DiffLine::Removed("old"),
            DiffLine::Added("new"),
        ];
        let mut output = Vec::new();

        write_colored_diff(&diff, &mut output).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!("  same\n{RED}- old{RESET}\n{GREEN}+ new{RESET}\n")
        );
    }

    #[test]
    fn ignores_hunk_header_line_number_changes() {
        let before = "diff --git a/file b/file\n@@ -1,3 +1,3 @@\n same\n-old\n+new\n";
        let after = "diff --git a/file b/file\n@@ -10,3 +10,3 @@\n same\n-old\n+new\n";

        assert_eq!(
            diff_normalized_lines(before, after),
            vec![
                DiffLine::Equal("diff --git a/file b/file"),
                DiffLine::Equal("@@ -1,3 +1,3 @@"),
                DiffLine::Equal(" same"),
                DiffLine::Equal("-old"),
                DiffLine::Equal("+new"),
            ]
        );
    }

    #[test]
    fn ignores_git_index_hash_updates() {
        let before =
            "diff --git a/file b/file\nindex 4fd408798..0b1103716 100644\n--- a/file\n+++ b/file\n";
        let after =
            "diff --git a/file b/file\nindex 4fd408798..95726d6dc 100644\n--- a/file\n+++ b/file\n";

        assert_eq!(
            diff_normalized_lines(before, after),
            vec![
                DiffLine::Equal("diff --git a/file b/file"),
                DiffLine::Equal("index 4fd408798..0b1103716 100644"),
                DiffLine::Equal("--- a/file"),
                DiffLine::Equal("+++ b/file"),
            ]
        );
    }

    #[test]
    fn preserves_git_index_mode_changes() {
        let before = "diff --git a/file b/file\nindex 4fd408798..0b1103716 100644\n";
        let after = "diff --git a/file b/file\nindex 4fd408798..95726d6dc 100755\n";

        assert_eq!(
            diff_normalized_lines(before, after),
            vec![
                DiffLine::Equal("diff --git a/file b/file"),
                DiffLine::Removed("index 4fd408798..0b1103716 100644"),
                DiffLine::Added("index 4fd408798..95726d6dc 100755"),
            ]
        );
    }

    #[test]
    fn omits_file_sections_without_diff_of_diff_changes() {
        let diff = vec![
            DiffLine::Equal("diff --git a/unchanged b/unchanged"),
            DiffLine::Equal("index 1111111..2222222 100644"),
            DiffLine::Equal("--- a/unchanged"),
            DiffLine::Equal("+++ b/unchanged"),
            DiffLine::Equal("@@ -1 +1 @@"),
            DiffLine::Equal("-old"),
            DiffLine::Equal("+new"),
            DiffLine::Equal("diff --git a/changed b/changed"),
            DiffLine::Equal("index 3333333..4444444 100644"),
            DiffLine::Removed("-old"),
            DiffLine::Added("-new"),
        ];
        let changed = changed_file_sections(&diff);
        let mut output = Vec::new();

        write_colored_diff_refs(&changed, &mut output).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!(
                "  diff --git a/changed b/changed\n  index 3333333..4444444 100644\n{RED}- -old{RESET}\n{GREEN}+ -new{RESET}\n"
            )
        );
    }

    #[test]
    fn compacts_long_unchanged_runs_in_output() {
        let diff = vec![
            DiffLine::Equal("diff --git a/changed b/changed"),
            DiffLine::Equal("line 1"),
            DiffLine::Equal("line 2"),
            DiffLine::Equal("line 3"),
            DiffLine::Equal("line 4"),
            DiffLine::Equal("line 5"),
            DiffLine::Equal("line 6"),
            DiffLine::Equal("line 7"),
            DiffLine::Equal("line 8"),
            DiffLine::Removed("-old"),
            DiffLine::Added("-new"),
        ];
        let changed = changed_file_sections(&diff);
        let mut output = Vec::new();

        write_compact_colored_diff_refs(&changed, &mut output).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!(
                "  diff --git a/changed b/changed\n  line 1\n  line 2\n  line 6\n  line 7\n  line 8\n{RED}- -old{RESET}\n{GREEN}+ -new{RESET}\n"
            )
        );
    }

    #[test]
    fn keeps_file_header_when_compacting_across_file_sections() {
        let diff = vec![
            DiffLine::Equal("diff --git a/first b/first"),
            DiffLine::Removed("-old first"),
            DiffLine::Added("-new first"),
            DiffLine::Equal("first tail 1"),
            DiffLine::Equal("first tail 2"),
            DiffLine::Equal("first tail 3"),
            DiffLine::Equal("diff --git a/second b/second"),
            DiffLine::Equal("second context 1"),
            DiffLine::Equal("second context 2"),
            DiffLine::Equal("second context 3"),
            DiffLine::Equal("second context 4"),
            DiffLine::Equal("second context 5"),
            DiffLine::Removed("-old second"),
            DiffLine::Added("-new second"),
        ];
        let changed = changed_file_sections(&diff);
        let mut output = Vec::new();

        write_compact_colored_diff_refs(&changed, &mut output).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!(
                "  diff --git a/first b/first\n{RED}- -old first{RESET}\n{GREEN}+ -new first{RESET}\n  first tail 1\n  first tail 2\n  first tail 3\n  diff --git a/second b/second\n  second context 3\n  second context 4\n  second context 5\n{RED}- -old second{RESET}\n{GREEN}+ -new second{RESET}\n"
            )
        );
    }

    #[test]
    fn parses_submodule_commit_ranges() {
        let diff = "\
diff --git a/libs/dep b/libs/dep
index 1111111..2222222 160000
--- a/libs/dep
+++ b/libs/dep
@@ -1 +1 @@
-Subproject commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
+Subproject commit bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
";

        assert_eq!(
            submodule_commit_ranges(diff),
            vec![SubmoduleCommitRange {
                path: "libs/dep".to_string(),
                old_commit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                new_commit: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            }]
        );
    }

    #[test]
    fn builds_submodule_patch_sets_from_before_and_after_diffs() {
        let before = "\
diff --git a/libs/dep b/libs/dep
index 1111111..2222222 160000
@@ -1 +1 @@
-Subproject commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
+Subproject commit bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
";
        let after = "\
diff --git a/libs/dep b/libs/dep
index 1111111..3333333 160000
@@ -1 +1 @@
-Subproject commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
+Subproject commit cccccccccccccccccccccccccccccccccccccccc
";

        assert_eq!(
            submodule_patch_sets(before, after),
            vec![SubmodulePatchSet {
                path: "libs/dep".to_string(),
                before_range: Some(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa..bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                        .to_string(),
                ),
                after_range: Some(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa..cccccccccccccccccccccccccccccccccccccccc"
                        .to_string(),
                ),
            }]
        );
    }
}
