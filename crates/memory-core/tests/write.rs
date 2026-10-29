use anyhow::Result;
use memory_core::{content_hash, read_markdown, write_markdown, WriteMode};
use std::{
    fs,
    sync::{Arc, Barrier},
};

#[test]
fn create_update_conflict_and_path_validation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let initial = "---\nid: decision\n---\n# Решение\nПервый вариант\n";
    let hash = write_markdown(&root, "decisions/auth.md", initial, WriteMode::Create)?;
    assert_eq!(hash, content_hash(initial.as_bytes()));
    assert_eq!(read_markdown(&root, "decisions/auth.md")?, initial);
    assert!(write_markdown(&root, "decisions/auth.md", "overwrite", WriteMode::Create).is_err());
    let updated = "---\nid: decision\n---\n# Решение\nВторой вариант\n";
    write_markdown(
        &root,
        "decisions/auth.md",
        updated,
        WriteMode::Update {
            expected_hash: &hash,
        },
    )?;
    assert!(write_markdown(
        &root,
        "decisions/auth.md",
        "stale update",
        WriteMode::Update {
            expected_hash: &hash
        }
    )
    .is_err());
    assert_eq!(fs::read_to_string(root.join("decisions/auth.md"))?, updated);
    for path in [
        "../escape.md",
        "/escape.md",
        "a/../escape.md",
        "a//b.md",
        "a/./b.md",
        "a\\b.md",
        "C:/escape.md",
        "a.md:stream",
        ".git/a.md",
        "CON.md",
        "a.txt",
        "a.md/",
        "a.md ",
    ] {
        assert!(
            write_markdown(&root, path, "content", WriteMode::Create).is_err(),
            "accepted {path}"
        );
    }
    assert!(write_markdown(&root, "empty.md", " \n", WriteMode::Create).is_err());
    assert!(write_markdown(
        &root,
        "huge.md",
        &"x".repeat(1024 * 1024 + 1),
        WriteMode::Create
    )
    .is_err());
    assert!(fs::read_dir(root.join("decisions"))?.all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
    Ok(())
}

#[test]
fn competing_writers_do_not_overwrite_each_other() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let hash = write_markdown(&root, "note.md", "# Before", WriteMode::Create)?;
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = ["# First", "# Second"]
        .into_iter()
        .map(|content| {
            let root = root.clone();
            let hash = hash.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                write_markdown(
                    &root,
                    "note.md",
                    content,
                    WriteMode::Update {
                        expected_hash: &hash,
                    },
                )
            })
        })
        .collect();
    let successes = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(Result::is_ok)
        .count();
    assert_eq!(successes, 1);
    assert!(["# First", "# Second"].contains(&read_markdown(&root, "note.md")?.as_str()));
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlink_targets_and_parents_cannot_escape() -> Result<()> {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("source");
    let outside = temp.path().join("outside");
    fs::create_dir(&root)?;
    fs::create_dir(&outside)?;
    let root = root.canonicalize()?;
    fs::write(outside.join("note.md"), "# Outside")?;
    symlink(&outside, root.join("linked"))?;
    symlink(outside.join("note.md"), root.join("note.md"))?;
    assert!(write_markdown(&root, "linked/escape.md", "bad", WriteMode::Create).is_err());
    assert!(write_markdown(
        &root,
        "note.md",
        "bad",
        WriteMode::Update {
            expected_hash: &content_hash(b"# Outside")
        }
    )
    .is_err());
    assert!(read_markdown(&root, "note.md").is_err());
    assert_eq!(fs::read_to_string(outside.join("note.md"))?, "# Outside");
    assert!(!outside.join("escape.md").exists());
    Ok(())
}
