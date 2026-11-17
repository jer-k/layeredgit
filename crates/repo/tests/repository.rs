//! Integration tests against disposable Git worktrees.

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    reason = "Fixture setup and expectations may fail the test"
)]

use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use repo::{
    BinaryReason, ChangeKind, ChangedFile, Contents, Error, FileKind, Layer, LineCounts, Repo,
    decode_text,
};

const ISOLATED: &str = "GIT_VIEWER_REPO_TEST_ISOLATED";

// Re-exec each test so both fixture and library subprocesses inherit the
// isolation settings, without mutating the process environment in parallel.
macro_rules! test_case {
    ($name:ident, $body:block) => {
        #[test]
        fn $name() {
            if isolated(stringify!($name)) {
                $body
            }
        }
    };
}

fn isolated(name: &str) -> bool {
    if std::env::var_os(ISOLATED).is_some() {
        return true;
    }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .env(ISOLATED, "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "0")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_PREFIX")
        .env("GIT_AUTHOR_NAME", "Repo Tests")
        .env("GIT_AUTHOR_EMAIL", "repo@example.test")
        .env("GIT_COMMITTER_NAME", "Repo Tests")
        .env("GIT_COMMITTER_EMAIL", "repo@example.test")
        .env("LC_ALL", "C");
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "isolated {name} failed\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

struct Fixture {
    directory: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        // A newline in the root also checks rev-parse path framing.
        let root = directory.path().join("working tree\n");
        fs::create_dir(&root).unwrap();
        let fixture = Self { directory, root };
        fixture.git(&["init", "--template="]);
        fixture
    }

    fn git(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.git_input(args, b""))
            .trim()
            .to_owned()
    }

    fn git_input(&self, args: &[&str], input: &[u8]) -> Vec<u8> {
        git_in(&self.root, args, input)
    }

    fn write(&self, path: impl AsRef<Path>, bytes: &[u8]) {
        let full_path = self.root.join(path);
        fs::create_dir_all(full_path.parent().unwrap()).unwrap();
        fs::write(full_path, bytes).unwrap();
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "-m", message]);
    }

    fn branch(&self) {
        self.git(&["switch", "-c", "feature"]);
    }

    fn open(&self) -> Repo {
        Repo::open(&self.root).unwrap()
    }
}

fn git_in(root: &Path, args: &[&str], input: &[u8]) -> Vec<u8> {
    let mut child = Command::new("git")
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "advice.addEmbeddedRepo=false",
        ])
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn file<'a>(files: &'a [ChangedFile], path: &str) -> &'a ChangedFile {
    files
        .iter()
        .find(|file| file.path() == Some(Path::new(path)))
        .unwrap()
}

test_case!(joins_committed_and_uncommitted_edits, {
    let fixture = Fixture::new();
    for path in ["committed", "working", "both", "deleted"] {
        fixture.write(path, b"old\n");
    }
    fixture.commit("base");
    fixture.branch();
    fixture.write("committed", b"old\ncommitted\n");
    fixture.write("both", b"committed\n");
    fixture.commit("branch edits");

    fixture.write("working", b"staged\n");
    fixture.git(&["add", "working"]);
    fixture.write("working", b"staged\nunstaged\n");
    fixture.write("both", b"committed\nunstaged\n");
    fixture.write("untracked", b"one line");
    fs::remove_file(fixture.root.join("deleted")).unwrap();

    let mut repo = fixture.open();
    let revisions = repo.revisions(None).unwrap();
    let files = repo.changed_files(&revisions).unwrap();
    let paths: Vec<_> = files
        .iter()
        .map(|file| file.path().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        ["both", "committed", "deleted", "untracked", "working"]
    );

    let committed = file(&files, "committed");
    assert_eq!(committed.committed.kind, ChangeKind::Modified);
    assert_eq!(
        committed.committed.lines,
        Some(LineCounts {
            added: 1,
            deleted: 0
        })
    );
    assert_eq!(committed.uncommitted.kind, ChangeKind::None);
    let working = file(&files, "working");
    assert_eq!(working.committed.kind, ChangeKind::None);
    assert_eq!(working.uncommitted.kind, ChangeKind::Modified);
    assert_eq!(
        working.uncommitted.lines,
        Some(LineCounts {
            added: 2,
            deleted: 1
        })
    );
    assert_eq!(
        repo.read_index(Path::new("working")).unwrap(),
        Contents::Bytes(b"staged\n".to_vec())
    );
    let both = file(&files, "both");
    assert_eq!(both.committed.kind, ChangeKind::Modified);
    assert_eq!(both.uncommitted.kind, ChangeKind::Modified);
    assert_eq!(
        both.uncommitted.lines,
        Some(LineCounts {
            added: 1,
            deleted: 0
        })
    );
    let deleted = file(&files, "deleted");
    assert_eq!(deleted.uncommitted.kind, ChangeKind::Deleted);
    assert!(deleted.worktree.is_none());
    assert_eq!(
        repo.read(&revisions, deleted, Layer::Worktree).unwrap(),
        Contents::Missing
    );
    let untracked = file(&files, "untracked");
    assert_eq!(untracked.uncommitted.kind, ChangeKind::Added);
    assert_eq!(
        untracked.uncommitted.lines,
        Some(LineCounts {
            added: 1,
            deleted: 0
        })
    );
    assert!(untracked.base.is_none() && untracked.head.is_none());
});

test_case!(compares_using_merge_base, {
    let fixture = Fixture::new();
    fixture.write("common", b"common\n");
    fixture.commit("fork point");
    let fork = fixture.git(&["rev-parse", "HEAD"]);
    fixture.branch();
    fixture.write("feature-file", b"feature\n");
    fixture.commit("feature commit");
    fixture.git(&["switch", "main"]);
    fixture.write("main-file", b"main\n");
    fixture.write("common", b"main changed this\n");
    fixture.commit("advance main");
    fixture.git(&["switch", "feature"]);

    let repo = fixture.open();
    let revisions = repo.revisions(Some(OsStr::new("main"))).unwrap();
    assert_eq!(revisions.merge_base(), Some(fork.as_str()));
    assert_ne!(
        revisions.merge_base(),
        Some(fixture.git(&["rev-parse", "main"]).as_str())
    );
    let files = repo.changed_files(&revisions).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(
        files.first().unwrap().path(),
        Some(Path::new("feature-file"))
    );
});

test_case!(follows_two_renames_and_reads_all_layers, {
    let fixture = Fixture::new();
    let original = "from space";
    let head = "中\nhead\t";
    let working = "0:work tree";
    let base_text = b"stable line one\nstable line two\nstable line three\n";
    fixture.write(original, base_text);
    fixture.commit("base");
    fixture.branch();

    fixture.git(&["mv", original, head]);
    let head_text = [base_text.as_slice(), b"head\n"].concat();
    fixture.write(head, &head_text);
    fixture.commit("rename and edit");
    fixture.git(&["mv", head, working]);
    let index_text = [head_text.as_slice(), b"index\n"].concat();
    fixture.write(working, &index_text);
    fixture.git(&["add", "--", working]);
    fixture.write("work tree", b"stage expression decoy");
    fixture.git(&["add", "--", "work tree"]);
    let worktree_text = [index_text.as_slice(), b"worktree\n"].concat();
    fixture.write(working, &worktree_text);
    // Reusing a former name creates a different file identity.
    fixture.write(head, b"untracked at the intermediate name\n");

    // The library explicitly enables renames and neutralizes output settings.
    fixture.git(&["config", "diff.renames", "false"]);
    fixture.git(&["config", "diff.relative", "true"]);
    fixture.git(&["config", "diff.external", "/does-not-exist"]);
    fixture.git(&["config", "diff.orderFile", "/does-not-exist"]);
    fixture.git(&["config", "color.ui", "always"]);

    let mut repo = fixture.open();
    let revisions = repo.revisions(None).unwrap();
    let files = repo.changed_files(&revisions).unwrap();
    let renamed = file(&files, working);
    assert_eq!(renamed.base.as_ref().unwrap().path, Path::new(original));
    assert_eq!(renamed.head.as_ref().unwrap().path, Path::new(head));
    assert_eq!(renamed.worktree.as_ref().unwrap().path, Path::new(working));
    assert_eq!(renamed.committed.kind, ChangeKind::Renamed);
    assert_eq!(renamed.uncommitted.kind, ChangeKind::Renamed);
    assert_eq!(
        renamed.committed.lines,
        Some(LineCounts {
            added: 1,
            deleted: 0
        })
    );
    assert_eq!(
        renamed.uncommitted.lines,
        Some(LineCounts {
            added: 2,
            deleted: 0
        })
    );
    assert_eq!(file(&files, head).uncommitted.kind, ChangeKind::Added);
    assert!(file(&files, head).head.is_none());

    assert_eq!(
        repo.read(&revisions, renamed, Layer::Base).unwrap(),
        Contents::Bytes(base_text.to_vec())
    );
    assert_eq!(
        repo.read(&revisions, renamed, Layer::Head).unwrap(),
        Contents::Bytes(head_text)
    );
    assert_eq!(repo.read_index(Path::new(head)).unwrap(), Contents::Missing);
    assert_eq!(
        repo.read_index(Path::new(working)).unwrap(),
        Contents::Bytes(index_text)
    );
    assert_eq!(
        repo.read(&revisions, renamed, Layer::Worktree).unwrap(),
        Contents::Bytes(worktree_text.clone())
    );
    fixture.git(&["add", "--", working]);
    assert_eq!(
        repo.read_index(Path::new(working)).unwrap(),
        Contents::Bytes(worktree_text)
    );
});

test_case!(preserves_non_utf8_path_bytes, {
    let fixture = Fixture::new();
    fixture.git(&["commit", "--allow-empty", "-m", "base"]);
    fixture.branch();
    let path = b"invalid-\xff\nname";
    let mut index_entry = b"100644 ".to_vec();
    let oid = fixture.git_input(&["hash-object", "-w", "--stdin"], b"content\n");
    index_entry.extend_from_slice(oid.strip_suffix(b"\n").unwrap());
    index_entry.push(b'\t');
    index_entry.extend_from_slice(path);
    index_entry.push(0);
    // Add directly to the index so this also works on filesystems that reject
    // undecodable names on disk.
    fixture.git_input(
        &["update-index", "--add", "-z", "--index-info"],
        &index_entry,
    );
    fixture.git(&["commit", "-m", "raw path"]);

    let mut repo = fixture.open();
    let revisions = repo.revisions(None).unwrap();
    let files = repo.changed_files(&revisions).unwrap();
    let entry = files.first().unwrap();
    assert_eq!(entry.path().unwrap().as_os_str().as_bytes(), path);
    assert_eq!(entry.committed.kind, ChangeKind::Added);
    assert_eq!(
        repo.read(&revisions, entry, Layer::Head).unwrap(),
        Contents::Bytes(b"content\n".to_vec())
    );
    assert_eq!(
        repo.read_index(Path::new(OsStr::from_bytes(path))).unwrap(),
        Contents::Bytes(b"content\n".to_vec())
    );
});

test_case!(binary_detection_and_utf8_are_distinct, {
    let fixture = Fixture::new();
    fixture.git(&["commit", "--allow-empty", "-m", "base"]);
    fixture.branch();
    fixture.write("binary", b"text\0data");
    fixture.write("invalid-utf8", b"line \xff\n");
    fixture.write("forced-binary", b"ordinary text\n");
    fixture.write(
        ".gitattributes",
        b"forced-binary -diff\nuntracked-binary diff=opaque\nuntracked-text diff\n",
    );
    fixture.commit("add files");
    fixture.git(&["config", "diff.opaque.binary", "true"]);
    fixture.git(&["config", "core.bigFileThreshold", "64"]);
    fixture.write("untracked-binary", b"also plain text\n");
    fixture.write("untracked-text", b"force\0text\n");
    fixture.write("over-git-threshold", &[b'a'; 65]);

    let mut repo = fixture.open();
    let revisions = repo.revisions(None).unwrap();
    let files = repo.changed_files(&revisions).unwrap();
    let binary = file(&files, "binary");
    assert!(binary.is_binary());
    assert_eq!(binary.committed.lines, None);
    assert!(file(&files, "forced-binary").is_binary());
    assert!(file(&files, "untracked-binary").uncommitted.binary);
    assert!(file(&files, "over-git-threshold").uncommitted.binary);
    assert!(!file(&files, "untracked-text").uncommitted.binary);
    let non_utf8 = file(&files, "invalid-utf8");
    assert!(!non_utf8.is_binary());

    for (entry, reason) in [
        (binary, BinaryReason::NulByte),
        (non_utf8, BinaryReason::InvalidUtf8),
    ] {
        let Contents::Bytes(bytes) = repo.read(&revisions, entry, Layer::Head).unwrap() else {
            panic!("expected blob bytes");
        };
        assert_eq!(decode_text(&bytes), Err(reason));
    }
    let mut boundary = vec![b'a'; 8000];
    boundary.push(0);
    assert!(decode_text(&boundary).is_ok());
    assert_eq!(decode_text(&boundary[1..]), Err(BinaryReason::NulByte));
});

test_case!(symlinks_and_submodules_have_distinct_file_kinds, {
    let fixture = Fixture::new();
    fixture.write("link", b"regular file\n");
    fixture.commit("base");
    fixture.branch();
    fs::remove_file(fixture.root.join("link")).unwrap();
    symlink("target\npath", fixture.root.join("link")).unwrap();

    let module = fixture.root.join("module");
    fs::create_dir(&module).unwrap();
    git_in(&module, &["init", "--template="], b"");
    git_in(
        &module,
        &["commit", "--allow-empty", "-m", "module commit"],
        b"",
    );
    fixture.commit("change kind and add gitlink");

    let mut repo = fixture.open();
    let revisions = repo.revisions(None).unwrap();
    let files = repo.changed_files(&revisions).unwrap();
    let link = file(&files, "link");
    assert_eq!(link.committed.kind, ChangeKind::TypeChanged);
    assert_eq!(link.base.as_ref().unwrap().kind, FileKind::Regular);
    assert_eq!(link.head.as_ref().unwrap().kind, FileKind::Symlink);
    for layer in [Layer::Head, Layer::Worktree] {
        assert_eq!(
            repo.read(&revisions, link, layer).unwrap(),
            Contents::Bytes(b"target\npath".to_vec())
        );
    }
    assert_eq!(
        repo.read_index(Path::new("link")).unwrap(),
        Contents::Bytes(b"target\npath".to_vec())
    );
    let gitlink = file(&files, "module");
    assert_eq!(gitlink.head.as_ref().unwrap().kind, FileKind::Submodule);
    for layer in [Layer::Head, Layer::Worktree] {
        assert_eq!(
            repo.read(&revisions, gitlink, layer).unwrap(),
            Contents::Submodule
        );
    }
    assert_eq!(
        repo.read_index(Path::new("module")).unwrap(),
        Contents::Submodule
    );
});

test_case!(unborn_head_has_empty_committed_layers, {
    let fixture = Fixture::new();
    fixture.write("staged", b"index\n");
    fixture.git(&["add", "staged"]);
    fixture.write("staged", b"index\nworktree\n");
    fixture.write("loose", b"untracked\n");

    let mut repo = fixture.open();
    let revisions = repo.revisions(None).unwrap();
    assert_eq!(revisions.head(), None);
    assert_eq!(revisions.merge_base(), None);
    let files = repo.changed_files(&revisions).unwrap();
    assert_eq!(files.len(), 2);
    for entry in &files {
        assert_eq!(entry.committed.kind, ChangeKind::None);
        assert_eq!(entry.uncommitted.kind, ChangeKind::Added);
        assert!(entry.base.is_none() && entry.head.is_none());
    }
    assert_eq!(
        file(&files, "staged").uncommitted.lines,
        Some(LineCounts {
            added: 2,
            deleted: 0
        })
    );
    assert_eq!(
        repo.read_index(Path::new("staged")).unwrap(),
        Contents::Bytes(b"index\n".to_vec())
    );
    assert_eq!(
        repo.read(&revisions, file(&files, "staged"), Layer::Base)
            .unwrap(),
        Contents::Missing
    );
});

test_case!(defaults_prefer_origin_then_main_then_master, {
    let fixture = Fixture::new();
    fixture.git(&["commit", "--allow-empty", "-m", "base"]);
    fixture.git(&["branch", "master"]);
    fixture.branch();
    let repo = fixture.open();
    assert_eq!(repo.default_base().unwrap(), "refs/heads/main");
    fixture.git(&["update-ref", "refs/remotes/origin/trunk", "HEAD"]);
    fixture.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/trunk",
    ]);
    assert_eq!(repo.default_base().unwrap(), "refs/remotes/origin/trunk");
    fixture.git(&["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
    fixture.git(&["branch", "-D", "main"]);
    assert_eq!(repo.default_base().unwrap(), "refs/heads/master");
    fixture.git(&["branch", "-D", "master"]);
    assert!(matches!(repo.default_base(), Err(Error::NoDefaultBase)));
    let error = repo.revisions(Some(OsStr::new("--invalid"))).unwrap_err();
    assert!(matches!(error, Error::Git { stderr, .. } if !stderr.is_empty()));
});

test_case!(discovers_from_a_file_and_reports_not_a_repo, {
    let fixture = Fixture::new();
    fixture.write("child/file", b"x");
    let repo = Repo::open(fixture.root.join("child/file")).unwrap();
    assert_eq!(repo.root(), fixture.root.canonicalize().unwrap());
    assert_eq!(
        repo.git_dir(),
        fixture.root.join(".git").canonicalize().unwrap()
    );
    assert!(matches!(
        Repo::open(fixture.directory.path()),
        Err(Error::NotRepository { .. })
    ));
});

test_case!(batch_reads_many_blobs_and_skips_large_versions, {
    let fixture = Fixture::new();
    fixture.git(&["commit", "--allow-empty", "-m", "base"]);
    fixture.branch();
    for index in 0..500 {
        fixture.write(
            format!("file-{index:03}"),
            format!("blob {index:03}\n").as_bytes(),
        );
    }
    fixture.write("aaa-large", &vec![b'a'; 65_536]);
    fixture.commit("many blobs");
    let mut repo = fixture.open();
    let revisions = repo.revisions(None).unwrap();
    let files = repo.changed_files(&revisions).unwrap();
    repo.set_byte_limit(9);
    let large = file(&files, "aaa-large");
    for layer in [Layer::Head, Layer::Worktree] {
        assert_eq!(
            repo.read(&revisions, large, layer).unwrap(),
            Contents::TooLarge {
                size: 65_536,
                limit: 9
            }
        );
    }
    assert_eq!(
        repo.read_index(Path::new("aaa-large")).unwrap(),
        Contents::TooLarge {
            size: 65_536,
            limit: 9
        }
    );
    for (index, entry) in files.iter().skip(1).enumerate() {
        let expected = Contents::Bytes(format!("blob {index:03}\n").into_bytes());
        assert_eq!(repo.read(&revisions, entry, Layer::Head).unwrap(), expected);
        assert_eq!(repo.read_index(entry.path().unwrap()).unwrap(), expected);
    }
    assert_eq!(files.len(), 501);
});

test_case!(polling_token_reports_status_index_and_head_changes, {
    let fixture = Fixture::new();
    let repo = fixture.open();
    let unborn = repo.change_token().unwrap();
    fixture.git(&["commit", "--allow-empty", "-m", "initial"]);
    let clean = repo.change_token().unwrap();
    assert_ne!(unborn, clean);
    assert_eq!(clean, repo.change_token().unwrap());

    fixture.write("new\nfile", b"untracked\n");
    let untracked = repo.change_token().unwrap();
    assert_ne!(clean, untracked);
    fixture.git(&["add", "--", "new\nfile"]);
    let staged = repo.change_token().unwrap();
    assert_ne!(untracked, staged);
    fixture.write("new\nfile", b"new staged bytes\n");
    fixture.git(&["add", "--", "new\nfile"]);
    assert_ne!(staged, repo.change_token().unwrap());
});

test_case!(recreated_paths_preserve_their_layer_presence, {
    let fixture = Fixture::new();
    fixture.write("committed-deletion", b"old\n");
    fixture.write("staged-deletion", b"old\n");
    fixture.commit("base");
    fixture.branch();
    fixture.git(&["rm", "committed-deletion"]);
    fixture.git(&["commit", "-m", "delete one"]);
    fixture.git(&["rm", "staged-deletion"]);
    fixture.write("committed-deletion", b"new\n");
    fixture.write("staged-deletion", b"replacement\n");

    let repo = fixture.open();
    let revisions = repo.revisions(None).unwrap();
    let files = repo.changed_files(&revisions).unwrap();
    assert_eq!(files.len(), 2);
    let committed = file(&files, "committed-deletion");
    assert!(committed.base.is_some() && committed.head.is_none() && committed.worktree.is_some());
    assert_eq!(committed.committed.kind, ChangeKind::Deleted);
    assert_eq!(committed.uncommitted.kind, ChangeKind::Added);
    let staged = file(&files, "staged-deletion");
    assert!(staged.base.is_some() && staged.head.is_some() && staged.worktree.is_some());
    assert_eq!(staged.uncommitted.kind, ChangeKind::Modified);
    assert_eq!(staged.uncommitted.lines, None);
});
