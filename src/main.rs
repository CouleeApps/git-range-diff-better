use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Write};
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
        range: String,
        status: String,
        stderr: String,
    },
    GitDiffIo {
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
                range,
                status,
                stderr,
            } => {
                writeln!(
                    f,
                    "git diff failed for range `{range}` with status {status}"
                )?;
                write!(f, "{}", stderr.trim_end())
            }
            AppError::GitDiffIo { range, source } => {
                write!(f, "failed to run git diff for range `{range}`: {source}")
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
    let before = git_diff(&options.before_range)?;
    let after = git_diff(&options.after_range)?;
    let diff = diff_normalized_lines(&before, &after);
    let changed_sections = changed_file_sections(&diff);

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    write_compact_colored_diff_refs(&changed_sections, &mut handle).map_err(AppError::Output)
}

struct Options {
    before_range: String,
    after_range: String,
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
         Each range is passed directly to `git diff --no-color --no-ext-diff <range>`."
    )
}

fn git_diff(range: &str) -> Result<String, AppError> {
    let output = Command::new("git")
        .args(["diff", "--no-color", "--no-ext-diff", range])
        .output()
        .map_err(|source| AppError::GitDiffIo {
            range: range.to_string(),
            source,
        })?;

    if !output.status.success() {
        return Err(AppError::GitDiffFailed {
            range: range.to_string(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

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

fn write_colored_diff(diff: &[DiffLine<'_>], writer: &mut impl Write) -> io::Result<()> {
    for line in diff {
        write_colored_line(line, writer)?;
    }

    Ok(())
}

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

    for line in &run[..UNCHANGED_CONTEXT_LINES] {
        write_colored_line(line, writer)?;
    }

    for line in &run[run.len() - UNCHANGED_CONTEXT_LINES..] {
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
}
