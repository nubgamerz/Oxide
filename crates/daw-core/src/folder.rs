//! A project's folder: everything the project makes or brings in lives inside it, so it is
//! self-contained (copy or move the folder and it opens anywhere) and nothing is written
//! elsewhere, temporary files included.
//!
//! `<Name>/<Name>.oxide` is the project file; beside it each kind of file has its folder,
//! made when first needed (opening an older project never changes its folder).

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// The project file's extension.
pub const EXTENSION: &str = "oxide";
/// Projects saved before the name: they still open, and save where they are.
pub const OLD_EXTENSION: &str = "dawproj";

/// Takes from recording (older projects recorded into `Audio/`, which still plays).
pub const RECORDED: &str = "Recorded";
/// The project's copy of every file dropped or imported: its working copy, so the original
/// is never touched.
pub const IMPORTED: &str = "Imported";
/// Rendered tracks and mixdowns.
pub const RENDERED: &str = "Rendered";
/// Where exports go unless the user picks elsewhere.
pub const EXPORTED: &str = "Exported";
/// Autosaved copies of the project file.
pub const BACKUPS: &str = "Backups";
/// Temporary files, which would otherwise go to the system's temp folder.
pub const TEMP: &str = "Temp";

/// Every folder a new project starts with.
pub const ALL: [&str; 6] = [RECORDED, IMPORTED, RENDERED, EXPORTED, BACKUPS, TEMP];

/// Whether `path` names a project file this app opens.
pub fn is_project_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION) || e.eq_ignore_ascii_case(OLD_EXTENSION))
}

/// A project name as a folder and file name: characters Windows refuses become `_`.
pub fn file_name(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Windows drops trailing dots and spaces from names.
    let s = s.trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() {
        "Untitled".into()
    } else {
        s
    }
}

/// Make the folder for a new project called `name` inside `parent`, with its subfolders, and
/// return the project file's path in it. Refuses a folder that already holds anything, so a
/// new project never mixes with other files.
pub fn create(parent: &Path, name: &str) -> Result<PathBuf> {
    if name.trim().is_empty() {
        bail!("a project needs a name");
    }
    if !parent.is_absolute() {
        bail!(
            "the folder for the project must be an absolute path, not {}",
            parent.display()
        );
    }
    let stem = file_name(name);
    let dir = parent.join(&stem);
    if dir.exists() {
        let busy = std::fs::read_dir(&dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .next()
            .is_some();
        if busy {
            bail!(
                "{} already exists and isn't empty: choose another name or folder",
                dir.display()
            );
        }
    }
    for sub in ALL {
        std::fs::create_dir_all(dir.join(sub))
            .with_context(|| format!("creating {}", dir.join(sub).display()))?;
    }
    Ok(dir.join(format!("{stem}.{EXTENSION}")))
}

/// A file name in `folder` nothing has yet: `<stem>.<ext>`, or `<stem> (2).<ext>` and on.
pub fn free_name(folder: &Path, stem: &str, ext: &str) -> PathBuf {
    (1..)
        .map(|n| match n {
            1 => folder.join(format!("{stem}.{ext}")),
            n => folder.join(format!("{stem} ({n}).{ext}")),
        })
        .find(|p| !p.exists())
        .expect("some name is free")
}

/// `<what> <local date time>`, as backups, renders and mixdowns are named.
pub fn stamped(what: &str) -> String {
    format!(
        "{} {}",
        file_name(what),
        chrono::Local::now().format("%Y-%m-%d %H.%M.%S")
    )
}

/// `dir/sub`, made if it isn't there yet.
pub fn ensure(dir: &Path, sub: &str) -> Result<PathBuf> {
    let path = dir.join(sub);
    std::fs::create_dir_all(&path).with_context(|| format!("creating {}", path.display()))?;
    Ok(path)
}

/// Whether two files hold the same bytes.
pub fn same_contents(a: &Path, b: &Path) -> Result<bool> {
    use std::io::Read;
    let (ma, mb) = (std::fs::metadata(a)?, std::fs::metadata(b)?);
    if ma.len() != mb.len() {
        return Ok(false);
    }
    let (mut fa, mut fb) = (std::fs::File::open(a)?, std::fs::File::open(b)?);
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let n = fa.read(&mut ba)?;
        if n == 0 {
            return Ok(true);
        }
        fb.read_exact(&mut bb[..n])?;
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
    }
}

/// Copy `source` into `folder` under its own name, or `name (2).ext` and on when that name
/// holds a different file; a copy already there with the same bytes is reused. Returns the
/// copy's path.
pub fn copy_into(source: &Path, folder: &Path) -> Result<PathBuf> {
    let name = source
        .file_name()
        .with_context(|| format!("{} has no file name", source.display()))?;
    let stem = source
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let ext = source
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    std::fs::create_dir_all(folder).with_context(|| format!("creating {}", folder.display()))?;
    for n in 1.. {
        let target = if n == 1 {
            folder.join(name)
        } else {
            folder.join(format!("{stem} ({n}){ext}"))
        };
        if !target.exists() {
            std::fs::copy(source, &target)
                .with_context(|| format!("copying {} to {}", source.display(), target.display()))?;
            return Ok(target);
        }
        if same_contents(source, &target)? {
            return Ok(target);
        }
    }
    unreachable!("some name is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide-folder-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_new_project_gets_its_folders_and_refuses_a_busy_one() {
        let parent = temp("create");
        let file = create(&parent, "My Song: Take 2").unwrap();
        assert_eq!(
            file,
            parent.join("My Song_ Take 2").join("My Song_ Take 2.oxide")
        );
        for sub in ALL {
            assert!(file.parent().unwrap().join(sub).is_dir(), "{sub}");
        }
        let err = create(&parent, "My Song: Take 2").unwrap_err().to_string();
        assert!(err.contains("isn't empty"), "{err}");
        assert!(create(&parent, " ").is_err());
        assert!(create(Path::new("relative"), "x").is_err());
        assert!(is_project_file(&file) && is_project_file(Path::new("a.DAWPROJ")));
        assert!(!is_project_file(Path::new("a.wav")));
        assert_eq!(file_name("  end. "), "end");
    }

    #[test]
    fn copies_are_numbered_and_reused() {
        let dir = temp("copy");
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("tone.wav"), b"one").unwrap();
        let into = dir.join("Imported");
        let first = copy_into(&a.join("tone.wav"), &into).unwrap();
        assert_eq!(first, into.join("tone.wav"));
        assert_eq!(
            copy_into(&a.join("tone.wav"), &into).unwrap(),
            first,
            "reused"
        );
        std::fs::write(b.join("tone.wav"), b"two").unwrap();
        let second = copy_into(&b.join("tone.wav"), &into).unwrap();
        assert_eq!(second, into.join("tone (2).wav"));
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        assert_eq!(
            std::fs::read(a.join("tone.wav")).unwrap(),
            b"one",
            "untouched"
        );
    }
}
