use std::fs;
use std::path::Path;
use std::process::Command;

use jevgrep::files::{chunk_lines, discover, fnmatch, read_lines, split_blocks, split_chunk, Chunk, Chunking, Discover};

fn lines_of(c: &Chunk) -> Vec<usize> {
    c.blocks.iter().flat_map(|&(a, b)| a..=b).collect()
}

#[test]
fn chunks_cover_every_line_once_with_leading_context() {
    let lines: Vec<String> = (1..=400).map(|i| format!("x{i} = {i}")).collect();
    let chunks = chunk_lines("f.py", &lines, Chunking { context: 10, ..Chunking::default() });
    assert_eq!((chunks[0].start, chunks.last().unwrap().end), (1, 400));
    assert!(chunks.windows(2).all(|w| w[0].end + 1 == w[1].start));
    assert!(chunks.iter().all(|c| c.end - c.start < 150));
    let second = &chunks[1];
    assert_eq!(second.ctx_start, second.start - 10);
    assert_eq!(second.text(second.start), format!("x{0} = {0}", second.start));
    assert_eq!(chunks.iter().flat_map(Chunk::askable).collect::<Vec<_>>(), (1..=400).collect::<Vec<_>>());
    assert_eq!(chunks.iter().flat_map(lines_of).collect::<Vec<_>>(), (1..=400).collect::<Vec<_>>());
}

const PY: &str = r#"import os

LIMIT = 3


class Greeter:
    """Says hello."""

    def __init__(self, name):
        self.name = name
        self.count = 0
        self.log = []
        self.extra = None
        self.more = None
        self.even_more = None
        self.last = None

    def greet(self):
        self.count += 1
        return f"hello {self.name}"


def main():
    g = Greeter(os.environ["USER"])
    print(g.greet())
    print(g.greet())
    print(g.greet())
    print(g.greet())
"#;

#[test]
fn split_blocks_follows_code_structure() {
    let py: Vec<&str> = PY.lines().collect();
    let blocks = split_blocks(&py);
    let starts: Vec<&str> = blocks.iter().map(|&(a, _)| py[a - 1].trim()).collect();
    assert!(starts.contains(&"class Greeter:") && starts.contains(&"def main():"));
    assert!(starts.contains(&"def greet(self):")); // a method becomes its own block once the class is long enough
    assert!(blocks.iter().all(|&(_, b)| !py[b - 1].trim().is_empty())); // no block ends on a blank line
    let covered: Vec<usize> = blocks.iter().flat_map(|&(a, b)| a..=b).collect();
    assert!(py.iter().enumerate().all(|(i, l)| l.trim().is_empty() || covered.contains(&(i + 1))));
}

#[test]
fn chunks_break_between_blocks_not_inside_them() {
    let mut lines = Vec::new();
    for i in 0..12 {
        lines.push(format!("def f{i}():"));
        lines.extend((0..20).map(|j| format!("    x{j} = {j}")));
        lines.push(String::new());
    }
    let chunks = chunk_lines("f.py", &lines, Chunking { max_lines: 100, ..Chunking::default() });
    assert!(chunks.len() > 1);
    assert!(chunks.iter().all(|c| lines[c.start - 1].starts_with("def ")));
}

#[test]
fn chunk_budget_shrinks_with_more_queries() {
    let lines = vec!["value = compute(thing)"; 300];
    let one = chunk_lines("f.py", &lines, Chunking { max_tokens: 5000, questions_per_line: 1.0, ..Chunking::default() });
    let nine = chunk_lines("f.py", &lines, Chunking { max_tokens: 5000, questions_per_line: 9.0, ..Chunking::default() });
    assert!(nine.len() > one.len());
    assert_eq!(nine.iter().map(|c| c.end - c.start + 1).sum::<usize>(), 300);
}

#[test]
fn askable_skips_blank_and_punctuation_lines() {
    let c = &chunk_lines("f.js", &["function f() {", "", "  return 1;", "}", "});"], Chunking::default())[0];
    assert_eq!(c.askable(), [1, 3]);
}

#[test]
fn long_lines_are_clipped_on_character_boundaries() {
    assert!(chunk_lines("f.txt", &["a".repeat(5000)], Chunking::default())[0].text(1).len() < 400);
    let wide = chunk_lines("f.txt", &["é".repeat(5000)], Chunking::default());
    assert_eq!(wide[0].text(1).chars().count(), 304);
}

#[test]
fn split_chunk_halves_at_a_block_boundary_and_preserves_text() {
    let lines: Vec<String> = (1..=100).map(|i| format!("l{i}")).collect();
    let c = &chunk_lines("f.py", &lines, Chunking::default())[0];
    let parts = split_chunk(c);
    let (a, b) = (&parts[0], &parts[1]);
    assert!(a.start == 1 && b.end == 100 && a.end + 1 == b.start);
    assert!(c.blocks.iter().any(|&(s, _)| s == b.start));
    assert_eq!((a.text(a.end), b.text(b.start), b.text(100)), (&*format!("l{}", a.end), &*format!("l{}", b.start), "l100"));
    assert!(b.ctx_start < b.start);
    assert_eq!(parts.iter().flat_map(lines_of).collect::<Vec<_>>(), (1..=100).collect::<Vec<_>>());
    let single = Chunk { path: "f".into(), ctx_start: 1, start: 1, end: 1, lines: vec!["x".into()], blocks: vec![] };
    assert!(split_chunk(&single).is_empty());
}

#[test]
fn fnmatch_follows_shell_rules_with_star_crossing_slashes() {
    assert!(fnmatch("src/a/b.py", "*.py") && fnmatch("src/a/b.py", "src/*") && fnmatch("b.py", "?.py"));
    assert!(fnmatch("a1.rs", "a[0-9].rs") && !fnmatch("ax.rs", "a[0-9].rs") && fnmatch("ax.rs", "a[!0-9].rs"));
    assert!(!fnmatch("b.pyc", "*.py") && !fnmatch("B.py", "b.py") && fnmatch("", "*") && !fnmatch("", "?"));
    assert!(fnmatch("a[b", "a[b") && fnmatch("docs/x/readme.md", "docs/*") && fnmatch("abcbc", "a*bc"));
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("jg-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, data: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

/// Discovery is relative to the cwd, which is process-wide, so these cases share one test.
#[test]
fn discover_honors_gitignore_skips_junk_and_works_without_git() {
    let repo = scratch("repo");
    assert!(Command::new("git").args(["init", "-q"]).current_dir(&repo).status().unwrap().success());
    write(&repo, ".gitignore", b"ignored/\n");
    for (rel, data) in [
        ("src/a.py", &b"print(1)\n"[..]),
        ("ignored/b.py", b"print(2)\n"),
        ("node_modules/x/i.js", b"x\n"),
        ("uv.lock", b"lock\n"),
        ("logo.png", b"\x89PNG"),
        ("empty.py", b""),
        ("docs/readme.md", b"# hi\n"),
    ] {
        write(&repo, rel, data);
    }
    std::env::set_current_dir(&repo).unwrap();
    let run = |paths: &[&str], opts: Discover| -> Vec<String> {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        let mut found: Vec<String> = discover(&paths, &opts).unwrap().iter().map(|p| jevgrep::files::display_path(p)).collect();
        found.sort();
        found
    };
    assert_eq!(run(&["."], Discover::default()), ["docs/readme.md", "src/a.py"]);
    assert_eq!(run(&[], Discover::default()), ["docs/readme.md", "src/a.py"]);
    assert_eq!(run(&["."], Discover { globs: vec!["*.py".into()], ..Discover::default() }), ["src/a.py"]);
    assert_eq!(run(&["."], Discover { excludes: vec!["docs/*".into()], ..Discover::default() }), ["src/a.py"]);
    assert_eq!(run(&[".", "src", "src/a.py"], Discover::default()), ["docs/readme.md", "src/a.py"]); // no duplicates
                                                                                                     // An explicitly named ignored directory is still searched.
    assert_eq!(run(&["ignored"], Discover::default()), ["ignored/b.py"]);
    // An explicitly named file always wins.
    assert_eq!(run(&["uv.lock"], Discover::default()), ["uv.lock"]);
    assert_eq!(discover(&["missing".to_string()], &Discover::default()), Err("missing".to_string()));
    // An absolute path inside the cwd is displayed relative to it.
    assert_eq!(run(&[repo.join("src").to_str().unwrap()], Discover::default()), ["src/a.py"]);

    let plain = scratch("plain");
    write(&plain, "a.py", b"x = 1\n");
    write(&plain, ".hidden.py", b"y = 2\n");
    std::env::set_current_dir(&plain).unwrap();
    assert_eq!(run(&["."], Discover { no_ignore: true, ..Discover::default() }), ["a.py"]);
    assert_eq!(run(&["."], Discover { no_ignore: true, hidden: true, ..Discover::default() }), [".hidden.py", "a.py"]);

    write(&plain, "b", b"ab\0cd");
    write(&plain, "t", b"one\r\ntwo\n");
    assert_eq!(read_lines(&plain.join("b")), None);
    assert_eq!(read_lines(&plain.join("t")), Some(vec!["one".to_string(), "two".to_string()]));
    std::env::set_current_dir(std::env::temp_dir()).unwrap();
    let _ = (fs::remove_dir_all(&repo), fs::remove_dir_all(&plain));
}
