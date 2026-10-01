//! Default configuration and library scaffolding.
use super::{PrompterError, config_path, info_message, library_dir, success_message};
use std::fs;
use std::path::{Path, PathBuf};
use tftio_lib::make_spinner;

/// Initialize default configuration and library structure.
///
/// # Errors
/// Returns an error if the home directory cannot be located, a config or
/// library directory cannot be created, or a default file cannot be written.
pub fn init_scaffold() -> Result<(), PrompterError> {
    let cfg_path = config_path()?;
    let lib = library_dir()?;
    scaffold_into(&cfg_path, &lib)
}

/// Create the default config file and library fragments under explicit roots.
///
/// Factored out of [`init_scaffold`] so the scaffolding logic can be driven
/// against an arbitrary directory in tests rather than the real `$HOME`.
/// `init_scaffold` supplies the home-derived config path and library root; the
/// behavior — idempotent creation and never overwriting an existing file — is
/// identical regardless of caller.
fn scaffold_into(cfg_path: &Path, lib: &Path) -> Result<(), PrompterError> {
    let pb = make_spinner(true, "Initializing prompter...");

    let cfg_dir = cfg_path
        .parent()
        .ok_or_else(|| PrompterError::NoParentDir(cfg_path.to_path_buf()))?;

    if let Some(ref pb) = pb {
        pb.set_message("Creating config directory...");
    }
    fs::create_dir_all(cfg_dir).map_err(|source| PrompterError::Io {
        path: cfg_dir.to_path_buf(),
        source,
    })?;

    if let Some(ref pb) = pb {
        pb.set_message("Creating library directory...");
    }
    fs::create_dir_all(lib).map_err(|source| PrompterError::Io {
        path: lib.to_path_buf(),
        source,
    })?;

    if !cfg_path.exists() {
        if let Some(ref pb) = pb {
            pb.set_message("Writing default config...");
        }
        let default_cfg = r#"# Prompter configuration
# Profiles map to sets of markdown files and/or other profiles.
# Files are relative to $HOME/.local/prompter/library

[python.api]
depends_on = ["a/b/c.md", "f/g/h.md"]

[general.testing]
depends_on = ["python.api", "a/b/d.md"]
"#;
        fs::write(cfg_path, default_cfg).map_err(|source| PrompterError::Io {
            path: cfg_path.to_path_buf(),
            source,
        })?;
    }

    let paths_and_contents: Vec<(PathBuf, &str)> = vec![
        (
            lib.join("a/b/c.md"),
            "# a/b/c.md\nExample snippet for python.api.\n",
        ),
        (lib.join("a/b.md"), "# a/b.md\nFolder-level notes.\n"),
        (
            lib.join("a/b/d.md"),
            "# a/b/d.md\nGeneral testing snippet.\n",
        ),
        (lib.join("f/g/h.md"), "# f/g/h.md\nShared helper snippet.\n"),
    ];

    for (path, contents) in paths_and_contents {
        if let Some(ref pb) = pb {
            pb.set_message(format!(
                "Creating {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| PrompterError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        if !path.exists() {
            fs::write(&path, contents).map_err(|source| PrompterError::Io {
                path: path.clone(),
                source,
            })?;
        }
    }

    if let Some(pb) = pb {
        pb.finish_with_message("Initialization complete!");
        std::thread::sleep(std::time::Duration::from_millis(200)); // Brief pause to show completion
    }

    println!(
        "{}",
        success_message(&format!("Initialized config at {}", cfg_path.display()))
    );
    println!(
        "{}",
        info_message(&format!("Library root at {}", lib.display()))
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Unique, per-call temp path that never collides across tests in this
    /// module. Mirrors the `src/types.rs` / `profile.rs` exemplar idiom.
    fn unique_temp_path(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "tftio-lib-scaffold-{label}-{}-{n}",
            std::process::id()
        ))
    }

    #[test]
    fn scaffold_into_creates_config_and_all_fragments() {
        let base = unique_temp_path("create");
        let cfg_path = base.join("config/config.toml");
        let lib = base.join("library");

        scaffold_into(&cfg_path, &lib).unwrap();

        assert!(cfg_path.is_file(), "config file should exist");
        let cfg_text = fs::read_to_string(&cfg_path).unwrap();
        assert!(cfg_text.contains("[python.api]"), "cfg={cfg_text}");
        assert!(cfg_text.contains("[general.testing]"), "cfg={cfg_text}");

        for rel in ["a/b/c.md", "a/b.md", "a/b/d.md", "f/g/h.md"] {
            assert!(lib.join(rel).is_file(), "missing fragment {rel}");
        }
        assert_eq!(
            fs::read_to_string(lib.join("a/b/c.md")).unwrap(),
            "# a/b/c.md\nExample snippet for python.api.\n"
        );

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scaffold_into_is_idempotent_and_never_overwrites() {
        let base = unique_temp_path("idempotent");
        let cfg_path = base.join("config/config.toml");
        let lib = base.join("library");

        // First run creates everything.
        scaffold_into(&cfg_path, &lib).unwrap();

        // A user-edited config and one user-edited fragment must survive a rerun.
        fs::write(&cfg_path, b"# user edited\n").unwrap();
        fs::write(lib.join("a/b/c.md"), b"USER FRAGMENT\n").unwrap();

        // The rerun is idempotent: it returns Ok and touches nothing existing.
        scaffold_into(&cfg_path, &lib).unwrap();

        assert_eq!(
            fs::read_to_string(&cfg_path).unwrap(),
            "# user edited\n",
            "existing config must not be overwritten"
        );
        assert_eq!(
            fs::read_to_string(lib.join("a/b/c.md")).unwrap(),
            "USER FRAGMENT\n",
            "existing fragment must not be overwritten"
        );
        // A fragment that was not user-touched remains present and unchanged.
        assert_eq!(
            fs::read_to_string(lib.join("f/g/h.md")).unwrap(),
            "# f/g/h.md\nShared helper snippet.\n"
        );

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scaffold_into_completes_a_partial_existing_tree() {
        let base = unique_temp_path("partial");
        let cfg_path = base.join("config/config.toml");
        let lib = base.join("library");

        // Pre-create the library root and one fragment subtree with custom
        // content; the config file does not yet exist.
        fs::create_dir_all(lib.join("a/b")).unwrap();
        fs::write(lib.join("a/b/c.md"), b"PREEXISTING\n").unwrap();

        scaffold_into(&cfg_path, &lib).unwrap();

        // The config is now written, the untouched fragments are filled in, and
        // the preexisting fragment is preserved.
        assert!(cfg_path.is_file());
        assert_eq!(
            fs::read_to_string(lib.join("a/b/c.md")).unwrap(),
            "PREEXISTING\n"
        );
        assert!(lib.join("f/g/h.md").is_file());
        assert!(lib.join("a/b/d.md").is_file());

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scaffold_into_errors_when_config_path_has_no_parent() {
        // The filesystem root has no parent directory.
        let lib = unique_temp_path("noparent-lib");
        let err = scaffold_into(Path::new("/"), &lib).unwrap_err();
        match err {
            PrompterError::NoParentDir(path) => assert_eq!(path, PathBuf::from("/")),
            other => panic!("expected NoParentDir, got {other:?}"),
        }
        // The library root was never reached, so nothing was created.
        assert!(!lib.exists());
    }

    #[test]
    fn scaffold_into_errors_when_config_dir_is_a_file() {
        let base = unique_temp_path("cfgdir-file");
        fs::create_dir_all(&base).unwrap();
        // `blocker` is a regular file, so creating it as a directory fails.
        let blocker = base.join("blocker");
        fs::write(&blocker, b"not a dir").unwrap();
        let cfg_path = blocker.join("config.toml");
        let lib = base.join("library");

        let err = scaffold_into(&cfg_path, &lib).unwrap_err();
        match err {
            PrompterError::Io { path, .. } => assert_eq!(path, blocker),
            other => panic!("expected Io error, got {other:?}"),
        }

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scaffold_into_errors_when_library_path_is_a_file() {
        let base = unique_temp_path("libpath-file");
        fs::create_dir_all(&base).unwrap();
        let cfg_path = base.join("config/config.toml");
        // The library path is an existing regular file, so creating it as a
        // directory fails.
        let lib = base.join("library-file");
        fs::write(&lib, b"not a dir").unwrap();

        let err = scaffold_into(&cfg_path, &lib).unwrap_err();
        match err {
            PrompterError::Io { path, .. } => assert_eq!(path, lib),
            other => panic!("expected Io error, got {other:?}"),
        }

        fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scaffold_into_errors_when_fragment_parent_creation_fails() {
        let base = unique_temp_path("frag-parent");
        let cfg_path = base.join("config/config.toml");
        let lib = base.join("library");
        // Make `<lib>/a` a regular file so creating `<lib>/a/b` fails once the
        // fragment loop is reached (after the config file is written).
        fs::create_dir_all(&lib).unwrap();
        fs::write(lib.join("a"), b"not a dir").unwrap();

        let err = scaffold_into(&cfg_path, &lib).unwrap_err();
        match err {
            PrompterError::Io { path, .. } => assert_eq!(path, lib.join("a/b")),
            other => panic!("expected Io error, got {other:?}"),
        }
        // The config file (written before the fragment loop) exists.
        assert!(cfg_path.is_file());

        fs::remove_dir_all(&base).ok();
    }

    /// Set the directory mode to read+execute only and confirm the bits truly
    /// block writes (they do not when the test runs as root). Returns `true`
    /// when the write-error arm can be exercised.
    #[cfg(unix)]
    fn readonly_dir_blocks_writes(dir: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let probe = dir.join(".probe");
        match fs::write(&probe, b"x") {
            Ok(()) => {
                fs::remove_file(&probe).ok();
                false
            }
            Err(_) => true,
        }
    }

    #[cfg(unix)]
    fn restore_writable(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).ok();
    }

    #[cfg(unix)]
    #[test]
    fn scaffold_into_surfaces_config_write_permission_error() {
        let base = unique_temp_path("cfg-write-perm");
        let cfg_dir = base.join("config");
        fs::create_dir_all(&cfg_dir).unwrap();
        let cfg_path = cfg_dir.join("config.toml");
        let lib = base.join("library");

        if !readonly_dir_blocks_writes(&cfg_dir) {
            restore_writable(&cfg_dir);
            fs::remove_dir_all(&base).ok();
            return;
        }

        let err = scaffold_into(&cfg_path, &lib).unwrap_err();
        restore_writable(&cfg_dir);
        match err {
            PrompterError::Io { path, .. } => assert_eq!(path, cfg_path),
            other => panic!("expected Io error, got {other:?}"),
        }

        fs::remove_dir_all(&base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn scaffold_into_surfaces_fragment_write_permission_error() {
        let base = unique_temp_path("frag-write-perm");
        let cfg_path = base.join("config/config.toml");
        let lib = base.join("library");
        // Pre-create the first fragment's parent so `create_dir_all(parent)` is
        // a no-op, then make it read-only so the `fs::write` for the fragment
        // fails.
        let frag_parent = lib.join("a/b");
        fs::create_dir_all(&frag_parent).unwrap();

        if !readonly_dir_blocks_writes(&frag_parent) {
            restore_writable(&frag_parent);
            fs::remove_dir_all(&base).ok();
            return;
        }

        let err = scaffold_into(&cfg_path, &lib).unwrap_err();
        restore_writable(&frag_parent);
        match err {
            PrompterError::Io { path, .. } => assert_eq!(path, lib.join("a/b/c.md")),
            other => panic!("expected Io error, got {other:?}"),
        }
        // The config file was written before the fragment loop reached failure.
        assert!(cfg_path.is_file());

        fs::remove_dir_all(&base).ok();
    }

    #[allow(unsafe_code)]
    fn set_home(value: &Path) -> Option<std::ffi::OsString> {
        let prior = std::env::var_os("HOME");
        // SAFETY: serialized by `env_lock`; the prior value is restored before
        // the test returns. Matches the sanctioned test-only env-mutation idiom
        // used elsewhere in this crate (REPO_INVARIANTS.md #5).
        unsafe {
            std::env::set_var("HOME", value);
        }
        prior
    }

    #[allow(unsafe_code)]
    fn restore_home(prior: Option<std::ffi::OsString>) {
        // SAFETY: serialized by `env_lock`; see `set_home`.
        unsafe {
            match prior {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    #[test]
    fn init_scaffold_writes_under_home_derived_paths() {
        let _guard = crate::promptlib::test_support::env_lock();
        let home = unique_temp_path("home");
        fs::create_dir_all(&home).unwrap();
        let prior = set_home(&home);

        let result = init_scaffold();

        // Restore HOME before any assertion so a failure cannot leak the
        // temp value into other tests sharing the process.
        restore_home(prior);
        result.unwrap();

        assert!(
            home.join(".config/prompter/config.toml").is_file(),
            "config should be created under the temp HOME"
        );
        assert!(
            home.join(".local/prompter/library/a/b/c.md").is_file(),
            "library fragment should be created under the temp HOME"
        );

        fs::remove_dir_all(&home).ok();
    }
}
