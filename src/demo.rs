//! Demo review with both layers populated and a large file for scroll checks.

use std::fmt::Write as _;

use crate::input::{ChangeKind, FileVersion, InputFile, ReviewInput};

use ChangeKind::{Added, Deleted, Modified, Renamed, Unchanged};

/// Builds the demo input used when opening the review window.
pub(crate) fn review() -> ReviewInput {
    ReviewInput {
        base_label: "main".into(),
        head_label: "feature/stacked-diffs".into(),
        files: vec![
            review_rs(),
            file(
                "web/review.ts",
                [
                    Some(
                        "export type Layer = \"committed\" | \"working\";\n\nexport function label(layer: Layer): string {\n  return layer;\n}\n",
                    ),
                    Some(
                        "export type Layer = \"committed\" | \"working\";\n\nexport function label(layer: Layer): string {\n  return layer === \"committed\" ? \"Committed\" : \"Uncommitted\";\n}\n",
                    ),
                    Some(
                        "export type Layer = \"committed\" | \"working\";\n\nexport function label(layer: Layer): string {\n  return layer === \"committed\" ? \"Committed\" : \"Uncommitted\";\n}\n",
                    ),
                ],
                [Modified, Unchanged],
            ),
            file(
                "review.toml",
                [
                    Some(
                        "[diff]\ncontext = 3\nshow_worktree = false\n\n[display]\nline_numbers = true\n",
                    ),
                    Some(
                        "[diff]\ncontext = 3\nshow_worktree = false\n\n[display]\nline_numbers = true\n",
                    ),
                    Some(
                        "[diff]\ncontext = 3\nshow_worktree = true\n\n[display]\nline_numbers = true\n",
                    ),
                ],
                [Unchanged, Modified],
            ),
            renamed_markdown(),
            file(
                "docs/keyboard.md",
                [
                    None,
                    Some(
                        "# Review shortcuts\n\n- `j` / `k` move between hunks.\n- `]` / `[` move between files.\n",
                    ),
                    Some(
                        "# Review shortcuts\n\n- `j` / `k` move between hunks.\n- `]` / `[` move between files.\n",
                    ),
                ],
                [Added, Unchanged],
            ),
            file(
                "src/legacy.rs",
                [
                    Some(
                        "// Old two-version review entry point.\npub fn legacy_diff() {\n    compare(\"main\", \"HEAD\");\n}\n",
                    ),
                    None,
                    None,
                ],
                [Deleted, Unchanged],
            ),
            binary(),
            file(
                "fixtures/eof.txt",
                [
                    Some("review-ready\n"),
                    Some("review-ready\n"),
                    Some("review-ready"),
                ],
                [Unchanged, Modified],
            ),
            generated_routes(),
        ],
    }
}

fn file(path: &str, texts: [Option<&str>; 3], layers: [ChangeKind; 2]) -> InputFile {
    let [base, head, worktree] = texts.map(|text| text.map(|text| version(path, Some(text))));
    let [committed, uncommitted] = layers;
    InputFile {
        base,
        head,
        worktree,
        binary: false,
        committed,
        uncommitted,
    }
}

fn version(path: &str, text: Option<&str>) -> FileVersion {
    FileVersion {
        path: path.into(),
        text: text.map(str::to_owned),
    }
}

fn review_rs() -> InputFile {
    let source = |preparation: &str, args: &str, default: &str| {
        format!(
            "use std::path::Path;\n\n\
             use crate::{{Review, ReviewMode}};\n\n\
             pub fn build_review(path: &Path, mode: ReviewMode) -> Review {{\n    \
             let base = resolve_base(path);\n    \
             let head = resolve_head(path);\n\
             {preparation}    Review::new({args})\n\
             }}\n\n\
             fn resolve_base(path: &Path) -> String {{\n    \
             let branch = find_default_branch(path);\n    \
             merge_base(path, &branch)\n\
             }}\n\n\
             fn resolve_head(path: &Path) -> String {{\n    \
             find_revision(path, \"HEAD\")\n\
             }}\n\n\
             /// Context used when a review opens.\n\
             pub const DEFAULT_CONTEXT: usize = {default};\n\n\
             /// Marker used for revisions that have no upstream.\n\
             pub const DETACHED_HEAD: &str = \"detached\";\n"
        )
    };
    let base = source(
        "    let files = read_patch(&base, &head);\n    let context = 1;\n",
        "files, mode, context",
        "1",
    );
    let head = source(
        "    let files = align_versions(&base, &head);\n    let trace = \"stack-preview\";\n    let context = 1;\n",
        "files, mode, context",
        "3",
    );
    let worktree = source(
        "    let files = align_versions(&base, &head);\n    let context = 3;\n    let worktree = read_worktree(path);\n",
        "files, mode, context, worktree",
        "3",
    );
    file(
        "src/review.rs",
        [Some(&base), Some(&head), Some(&worktree)],
        [Modified, Modified],
    )
}

fn renamed_markdown() -> InputFile {
    let before =
        "# Review notes\n\nStart with the branch comparison.\n\nSee [shortcuts](keyboard.md).\n";
    let after = "# Review guide\n\nStart with the **branch comparison** and check working-tree edits.\n\nSee [shortcuts](keyboard.md).\n";
    InputFile {
        base: Some(version("docs/review-notes.md", Some(before))),
        head: Some(version("docs/review-guide.md", Some(after))),
        worktree: Some(version("docs/review-guide.md", Some(after))),
        binary: false,
        committed: Renamed,
        uncommitted: Unchanged,
    }
}

fn binary() -> InputFile {
    let version = || Some(version("assets/review.png", None));
    InputFile {
        base: version(),
        head: version(),
        worktree: version(),
        binary: true,
        committed: Unchanged,
        uncommitted: Modified,
    }
}

fn generated_routes() -> InputFile {
    let routes = |version| {
        let mut text = String::from("export const routes = {\n");
        for id in 1..=5_000 {
            let prefix = if version >= 2 && id % 347 == 0 {
                "preview"
            } else if version >= 1 && id % 127 == 0 {
                "review"
            } else {
                "route"
            };
            let _ = writeln!(text, "  r{id}: \"/{prefix}/{id}\",");
        }
        text.push_str("};\n");
        text
    };
    file(
        "fixtures/generated-routes.ts",
        [Some(&routes(0)), Some(&routes(1)), Some(&routes(2))],
        [Modified, Modified],
    )
}
