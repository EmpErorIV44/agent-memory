//! Metadata-only polling hints. Successful index scans remain authoritative.
use anyhow::{bail, ensure, Context, Result};
use cap_std::{ambient_authority, fs::Dir};
use std::path::{Component, Path, PathBuf};

/// Fingerprint Markdown paths, lengths and modification times without reading
/// their contents. Errors are not empty-source/deletion signals. Callers should
/// keep the previous successful fingerprint and retry/reconcile after an error.
pub fn fingerprint(root: &Path) -> Result<String> {
    let directory = open_root(root)?;
    let mut files = Vec::new();
    visit(&directory, Path::new(""), &mut files)?;
    files.sort();
    Ok(memory_core::content_hash(&serde_json::to_vec(&files)?))
}

fn open_root(root: &Path) -> Result<Dir> {
    ensure!(root.is_absolute(), "source root must be absolute");
    let mut anchor = PathBuf::new();
    let mut names = Vec::new();
    for component in root.components() {
        match component {
            Component::Prefix(_) | Component::RootDir if names.is_empty() => {
                anchor.push(component.as_os_str());
            }
            Component::Normal(name) => names.push(name),
            _ => bail!("source root must be a canonical absolute path"),
        }
    }
    let mut directory = Dir::open_ambient_dir(anchor, ambient_authority())?.into_std_file();
    for name in names {
        directory = cap_primitives::fs::open_dir_nofollow(&directory, Path::new(name))
            .context("open polling source without following symbolic links")?;
    }
    Ok(Dir::from_std_file(directory))
}

fn visit(directory: &Dir, relative: &Path, files: &mut Vec<(String, u64, u128)>) -> Result<()> {
    for entry in directory.entries().context("enumerate polling source")? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".agent-memory-history" || name == ".agent-memory-operations" {
            continue;
        }
        // Metadata lookup never follows a final symlink, including a replacement
        // after directory enumeration. No Markdown file handle is opened.
        let metadata = directory.symlink_metadata(&name)?;
        if metadata.is_symlink() {
            continue;
        }
        let path = relative.join(&name);
        if metadata.is_dir() {
            let anchor = directory.try_clone()?.into_std_file();
            let child = Dir::from_std_file(cap_primitives::fs::open_dir_nofollow(
                &anchor,
                Path::new(&name),
            )?);
            visit(&child, &path, files)?;
        } else if metadata.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            let modified = metadata
                .modified()?
                .duration_since(cap_std::time::SystemTime::from_std(std::time::UNIX_EPOCH))?;
            files.push((
                path.to_str()
                    .context("Markdown polling path is not UTF-8")?
                    .replace('\\', "/"),
                metadata.len(),
                modified.as_nanos(),
            ));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{symlink, PermissionsExt},
    };

    #[test]
    fn detects_metadata_mutations_and_ignores_internal_journals() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let empty = fingerprint(&root)?;
        fs::write(root.join("note.md"), b"one")?;
        let added = fingerprint(&root)?;
        assert_ne!(added, empty);
        assert_eq!(fingerprint(&root)?, added);
        fs::write(root.join("note.md"), b"different length")?;
        let edited = fingerprint(&root)?;
        assert_ne!(edited, added);
        fs::rename(root.join("note.md"), root.join("renamed.MD"))?;
        let renamed = fingerprint(&root)?;
        assert_ne!(renamed, edited);
        fs::create_dir(root.join(".hidden"))?;
        fs::write(root.join(".hidden/note.md"), b"hidden source note")?;
        let hidden = fingerprint(&root)?;
        assert_ne!(hidden, renamed);
        for name in [".agent-memory-history", ".agent-memory-operations"] {
            fs::create_dir(root.join(name))?;
            fs::write(root.join(name).join("ignore.md"), b"internal")?;
        }
        fs::write(root.join("unrelated.txt"), b"unrelated")?;
        assert_eq!(fingerprint(&root)?, hidden);
        fs::remove_file(root.join("renamed.MD"))?;
        assert_ne!(fingerprint(&root)?, hidden);
        assert!(fingerprint(&root.join("missing")).is_err());
        assert!(fingerprint(Path::new("relative")).is_err());
        Ok(())
    }

    #[test]
    fn avoids_symlinks_and_does_not_need_to_read_note_contents() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("source");
        let outside = temp.path().join("outside");
        fs::create_dir(&root)?;
        fs::create_dir(&outside)?;
        fs::write(outside.join("secret.md"), b"outside")?;
        let root = root.canonicalize()?;
        let initial = fingerprint(&root)?;
        symlink(&outside, root.join("linked"))?;
        symlink(outside.join("secret.md"), root.join("linked.md"))?;
        assert_eq!(fingerprint(&root)?, initial);
        fs::write(outside.join("secret.md"), b"outside changed")?;
        assert_eq!(fingerprint(&root)?, initial);
        let alias = temp.path().join("alias");
        symlink(&root, &alias)?;
        assert!(fingerprint(&alias).is_err());
        fs::write(root.join("unreadable.md"), [0xff, 0xfe, 0xfd])?;
        fs::set_permissions(
            root.join("unreadable.md"),
            fs::Permissions::from_mode(0o000),
        )?;
        let before = fs::read_dir(&root)?.count();
        let first = fingerprint(&root)?;
        assert_ne!(first, initial);
        assert_eq!(fingerprint(&root)?, first);
        assert_eq!(fs::read_dir(&root)?.count(), before);
        fs::set_permissions(
            root.join("unreadable.md"),
            fs::Permissions::from_mode(0o600),
        )?;
        assert_eq!(fs::read(root.join("unreadable.md"))?, [0xff, 0xfe, 0xfd]);
        Ok(())
    }
}
