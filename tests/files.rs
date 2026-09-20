use std::fs;
use std::path::Path;
use std::process::Command;

use jevgrep::files::{chunk_lines, discover, fnmatch, is_definition, read_lines, split_blocks, split_chunk, Chunk, Chunking, Discover};

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

#[test]
fn fnmatch_character_classes_and_literals_are_table_driven() {
    let cases: &[(&str, &str, bool)] = &[
        ("", "", true),
        ("", "*", true),
        ("", "?", false),
        ("a", "", false),
        ("a", "a", true),
        ("a", "b", false),
        ("é", "?", true),
        ("🙂", "?", true),
        ("ab", "?", false),
        ("αβ", "??", true),
        ("src/a/b.py", "src/*/b.py", true),
        ("src/a/b.py", "src/*/*.py", true),
        ("a/b/c", "a/*", true),
        ("abc", "a*c", true),
        ("ac", "a*c", true),
        ("abbc", "a*bc", true),
        ("file.rs", "*.rs", true),
        ("file.rs.bak", "*.rs", false),
        ("a", "[abc]", true),
        ("d", "[abc]", false),
        ("b", "[a-c]", true),
        ("d", "[a-c]", false),
        ("0", "[!a-z]", true),
        ("q", "[!a-z]", false),
        ("3", "[^0-9]", false),
        ("x", "[^0-9]", true),
        ("]", "[]]", true),
        ("a", "[]]", false),
        ("a", "[[]", false),
        ("[", "[[]", true),
        ("Z", "[A-Z]", true),
        ("z", "[A-Z]", false),
        ("foo", "foo*", true),
        ("foobar", "foo*", true),
        ("barfoo", "foo*", false),
        ("ab", "a**b", true),
        ("axb", "a?b", true),
        ("ab", "a?b", false),
    ];
    for &(text, pattern, want) in cases {
        assert_eq!(fnmatch(text, pattern), want, "fnmatch({text:?}, {pattern:?})");
    }
}

/// A star-and-question pattern without classes is prefix/suffix/single-char matching.
fn star_question_oracle(text: &str, pattern: &str) -> bool {
    let (t, p): (Vec<char>, Vec<char>) = (text.chars().collect(), pattern.chars().collect());
    fn rec(t: &[char], p: &[char]) -> bool {
        match (t, p) {
            ([], p) => p.iter().all(|&c| c == '*'),
            (t, ['*', rest @ ..]) => rec(t, rest) || rec(&t[1..], p),
            ([_, t_rest @ ..], ['?', p_rest @ ..]) => rec(t_rest, p_rest),
            ([tc, t_rest @ ..], [pc, p_rest @ ..]) if tc == pc => rec(t_rest, p_rest),
            _ => false,
        }
    }
    rec(&t, &p)
}

#[test]
fn fnmatch_agrees_with_a_star_question_oracle() {
    let mut rng = 19u64;
    let step = |rng: &mut u64| {
        *rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*rng >> 33) as usize
    };
    let alphabet = ['a', 'b', 'c', '/', '.', 'é'];
    for _ in 0..300 {
        let text: String = (0..step(&mut rng) % 8).map(|_| alphabet[step(&mut rng) % alphabet.len()]).collect();
        let pattern: String = (0..step(&mut rng) % 8)
            .map(|_| match step(&mut rng) % 6 {
                0 => '*',
                1 => '?',
                n => alphabet[n % alphabet.len()],
            })
            .collect();
        assert_eq!(fnmatch(&text, &pattern), star_question_oracle(&text, &pattern), "{text:?} {pattern:?}");
    }
    assert!(fnmatch("anything", "*"));
    assert!(fnmatch("exact", "exact"));
    assert!(!fnmatch("exact", "other"));
}

#[test]
fn is_definition_recognizes_common_openers_and_rejects_lookalikes() {
    let cases: &[(&str, bool)] = &[
        ("def foo():", true),
        ("    def foo():", true),
        ("async def foo():", true),
        ("class Greeter:", true),
        ("pub fn foo() {", true),
        ("pub(crate) async fn run() {", true),
        ("export default function App() {", true),
        ("struct Walk {", true),
        ("impl Foo {", true),
        ("enum Kind {", true),
        ("trait Bar {", true),
        ("type Alias = u8;", true),
        ("mod inner {", true),
        ("macro_rules! m {", true),
        ("x = 1", false),
        ("define", false),
        ("default = 1", false),
        ("# def foo", false),
        ("// fn foo", false),
        ("fnord = 1", false),
        ("classic = 1", false),
    ];
    for &(line, want) in cases {
        assert_eq!(is_definition(line), want, "{line:?}");
    }
}

#[test]
fn clip_respects_character_boundaries_and_the_documented_budget() {
    use jevgrep::files::{clip, MAX_LINE_CHARS};
    assert_eq!(clip("short"), "short");
    assert_eq!(clip("short  \t"), "short");
    let ascii = "a".repeat(MAX_LINE_CHARS + 20);
    let clipped = clip(&ascii);
    assert!(clipped.ends_with(" ..."));
    assert_eq!(clipped.chars().count(), MAX_LINE_CHARS + 4);
    let wide = "界".repeat(MAX_LINE_CHARS + 8);
    let clipped = clip(&wide);
    assert_eq!(clipped.chars().count(), MAX_LINE_CHARS + 4);
    assert!(clipped.starts_with(&"界".repeat(MAX_LINE_CHARS)));
    let mixed = format!("{}🙂{}", "e\u{301}".repeat(10), "x".repeat(MAX_LINE_CHARS));
    let clipped = clip(&mixed);
    assert!(clipped.is_char_boundary(clipped.len().saturating_sub(4)) || clipped.ends_with(" ..."));
    assert!(clipped.chars().count() <= MAX_LINE_CHARS + 4);
}

#[test]
fn chunk_and_block_invariants_hold_for_arbitrary_files() {
    let mut rng = 23u64;
    let step = |rng: &mut u64| {
        *rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*rng >> 33) as usize
    };
    for _ in 0..80 {
        let n = 1 + step(&mut rng) % 80;
        let lines: Vec<String> = (0..n)
            .map(|i| match step(&mut rng) % 5 {
                0 => String::new(),
                1 => format!("def f{i}():"),
                2 => format!("    x{i} = {i}"),
                3 => "    # comment".into(),
                _ => format!("value_{i}"),
            })
            .collect();
        let blocks = split_blocks(&lines);
        assert!(blocks.windows(2).all(|w| w[0].1 < w[1].0), "blocks must be disjoint and ordered");
        for &(a, b) in &blocks {
            assert!(1 <= a && a <= b && b <= n);
            assert!(!lines[b - 1].trim().is_empty() || a == b);
        }
        let covered: Vec<usize> = blocks.iter().flat_map(|&(a, b)| a..=b).collect();
        assert!(lines.iter().enumerate().all(|(i, line)| line.trim().is_empty() || covered.contains(&(i + 1))));
        let cfg = Chunking { max_lines: 8 + step(&mut rng) % 40, context: step(&mut rng) % 6, ..Chunking::default() };
        let chunks = chunk_lines("f.py", &lines, cfg);
        let asked: Vec<usize> = chunks.iter().flat_map(|c| c.start..=c.end).collect();
        if lines.iter().any(|l| !l.trim().is_empty()) {
            assert!(!chunks.is_empty());
            assert!(asked.windows(2).all(|w| w[0] < w[1]), "chunk ranges cover each line at most once");
        }
        for c in &chunks {
            assert!(c.ctx_start <= c.start && c.start <= c.end);
            assert!(c.askable().iter().all(|&n| c.start <= n && n <= c.end));
            assert!(c.askable().iter().all(|&n| c.text(n).bytes().any(|b| b.is_ascii_alphanumeric())));
            assert!(c.end - c.start < cfg.max_lines.max(1));
            assert_eq!(c.lines.len(), c.end - c.ctx_start + 1);
        }
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("jg-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    // macOS TMPDIR can use /var while current_dir() resolves /private/var.
    // This fixture must use the same physical path as cwd for the absolute-path
    // assertion below; keep the relative display-path assertion unchanged.
    dir.canonicalize().unwrap()
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
