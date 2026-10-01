//! Profile rendering and output formatting.
use super::config::Config;
use super::config::load_bundle;
use super::profile::{FamilyName, resolve_profile_for_family};
use super::{
    FragmentOutput, Framing, PrompterError, RenderOutput, default_post_prompt, default_pre_prompt,
    format_system_prefix,
};
use chrono::Local;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tftio_lib::{JsonOutput, render_response};

/// Render one or more profiles to the given writer.
///
/// # Errors
///
/// Returns an error when profile resolution, fragment reading, serialization,
/// or writing to the supplied output fails.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "preserves the published prompter library surface; an options struct would be a breaking change"
)]
pub fn render_to_writer(
    cfg: &Config,
    mut w: impl Write,
    profiles: &[String],
    family: Option<&FamilyName>,
    separator: Option<&str>,
    pre_prompt: Option<&str>,
    post_prompt: Option<&str>,
    framing: Framing,
    output: JsonOutput,
) -> Result<(), PrompterError> {
    let mut seen_files = HashSet::new();
    let mut files: Vec<(PathBuf, PathBuf)> = Vec::new();

    // Resolve all profiles with shared deduplication
    for profile in profiles {
        let mut stack = Vec::new();
        resolve_profile_for_family(
            profile,
            cfg,
            family,
            &mut seen_files,
            &mut stack,
            &mut files,
        )?;
    }

    if output.is_json() {
        return write_json_render(&mut w, &files, profiles, pre_prompt, framing);
    }

    // Text output mode. Bare framing suppresses every auto-injected piece —
    // the default pre-prompt, the date/system prefix, and the default (and
    // config) post-prompt — leaving only the fragment bodies. An explicit
    // -p/-P still wins: the user asked for that exact text.
    let default_pre = default_pre_prompt();
    let pre_prompt_text = match (pre_prompt, framing) {
        (Some(explicit), _) => Some(explicit),
        (None, Framing::Full) => Some(default_pre.as_str()),
        (None, Framing::Bare) => None,
    };
    if let Some(text) = pre_prompt_text {
        w.write_all(text.as_bytes()).map_err(PrompterError::Write)?;
    }

    // Date/system stamp is auto-injected, so it appears only in full framing;
    // it is the cache-buster bare exists to drop.
    if framing.is_full() {
        w.write_all(b"\n").map_err(PrompterError::Write)?;
        let prefix = format_system_prefix();
        w.write_all(prefix.as_bytes())
            .map_err(PrompterError::Write)?;
    }

    let sep = separator.unwrap_or("");
    for (index, (path, _library_root)) in files.iter().enumerate() {
        // Newline before each file. In bare framing the leading newline is
        // dropped so the output begins directly with the first fragment.
        if framing.is_full() || index > 0 {
            w.write_all(b"\n").map_err(PrompterError::Write)?;
        }

        let bytes = fs::read(path).map_err(|source| PrompterError::Io {
            path: path.clone(),
            source,
        })?;
        w.write_all(&bytes).map_err(PrompterError::Write)?;

        // Write separator after each file if provided
        if !sep.is_empty() {
            w.write_all(sep.as_bytes()).map_err(PrompterError::Write)?;
        }
    }

    // Post-prompt: explicit -P wins. Full falls back to the config post-prompt
    // then the default; bare emits nothing without an explicit -P. The full
    // framing keeps its two-newline lead-in; bare writes the explicit text
    // verbatim so nothing is auto-injected.
    let default_post = default_post_prompt();
    let post_prompt_text = match framing {
        Framing::Full => Some(
            post_prompt
                .or(cfg.post_prompt.as_deref())
                .unwrap_or(&default_post),
        ),
        Framing::Bare => post_prompt,
    };
    if let Some(text) = post_prompt_text {
        if framing.is_full() {
            w.write_all(b"\n\n").map_err(PrompterError::Write)?;
        }
        w.write_all(text.as_bytes()).map_err(PrompterError::Write)?;
    }

    Ok(())
}

/// Render the deduplicated fragments as the shared JSON response envelope.
///
/// Bare framing drops the auto-injected default pre-prompt and the date/system
/// stamp, leaving the fragments as the payload; an explicit `pre_prompt` still
/// wins.
fn write_json_render(
    mut w: impl Write,
    files: &[(PathBuf, PathBuf)],
    profiles: &[String],
    pre_prompt: Option<&str>,
    framing: Framing,
) -> Result<(), PrompterError> {
    let pre_prompt_text = match pre_prompt {
        Some(explicit) => explicit.to_string(),
        None if framing.is_full() => default_pre_prompt(),
        None => String::new(),
    };

    let system_info = if framing.is_full() {
        let date = Local::now().format("%Y-%m-%d").to_string();
        let os = env::consts::OS;
        let arch = env::consts::ARCH;
        format!("Today is {date}, and you are running on a {arch}/{os} system.")
    } else {
        String::new()
    };

    let mut fragments = Vec::new();
    for (path, library_root) in files {
        let content = fs::read_to_string(path).map_err(|source| PrompterError::Io {
            path: path.clone(),
            source,
        })?;
        let rel_path = path
            .strip_prefix(library_root)
            .unwrap_or(path)
            .display()
            .to_string();
        fragments.push(FragmentOutput {
            path: rel_path,
            content,
        });
    }

    let payload = serde_json::to_value(RenderOutput {
        profile: profiles.join(", "),
        pre_prompt: pre_prompt_text,
        system_info,
        fragments,
    })?;
    writeln!(
        &mut w,
        "{}",
        render_response("run", JsonOutput::Json, payload, String::new())
    )
    .map_err(PrompterError::Write)
}

/// Render one or more profiles to stdout.
///
/// Convenience function that reads configuration and renders the specified
/// profiles to standard output with optional separator, pre-prompt, and post-prompt.
/// When multiple profiles are provided, files are deduplicated across all profiles.
///
/// # Arguments
/// * `profiles` - Profile names to render (deduplicated in order)
/// * `family` - Optional family used to substitute matching fragment variants
/// * `separator` - Optional separator between files
/// * `pre_prompt` - Optional custom pre-prompt text
/// * `post_prompt` - Optional custom post-prompt text
/// * `framing` - Whether to wrap fragments in framing context or emit bare bodies
/// * `config_override` - Optional configuration file override
/// * `json` - Whether to output in JSON format
///
/// # Errors
/// Returns an error if:
/// - Configuration file cannot be read or parsed
/// - Profile resolution fails
/// - Writing to stdout fails
#[allow(
    clippy::too_many_arguments,
    reason = "preserves the published prompter library surface; an options struct would be a breaking change"
)]
pub fn run_render_stdout(
    profiles: &[String],
    family: Option<&FamilyName>,
    separator: Option<&str>,
    pre_prompt: Option<&str>,
    post_prompt: Option<&str>,
    framing: Framing,
    config_override: Option<&Path>,
    output: JsonOutput,
) -> Result<(), PrompterError> {
    let (_cfg_path, cfg) = load_bundle(config_override)?;
    let stdout = io::stdout();
    let handle = stdout.lock();
    render_to_writer(
        &cfg,
        handle,
        profiles,
        family,
        separator,
        pre_prompt,
        post_prompt,
        framing,
        output,
    )
}

/// Render composed profiles to a byte vector.
///
/// Convenience wrapper around [`render_to_writer`] that handles config
/// resolution and returns the rendered output as bytes. Intended for
/// use by other crates that need prompt composition as a library.
///
/// # Arguments
/// * `profiles` - Profile names to compose
/// * `family` - Optional family used to substitute matching fragment variants
/// * `config_override` - Optional path to custom config file
///
/// # Errors
/// Returns an error if config resolution, profile resolution, or rendering fails.
pub fn render_to_vec(
    profiles: &[String],
    family: Option<&FamilyName>,
    config_override: Option<&Path>,
) -> Result<Vec<u8>, PrompterError> {
    let (_cfg_path, cfg) = load_bundle(config_override)?;
    let mut buf = Vec::new();
    render_to_writer(
        &cfg,
        &mut buf,
        profiles,
        family,
        None,
        None,
        None,
        Framing::Full,
        JsonOutput::Text,
    )?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::promptlib::config::ProfileDef;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Unique, per-call temp path that never collides across tests in this
    /// module. Mirrors the `src/types.rs` / `profile.rs` exemplar idiom.
    fn unique_temp_path(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "tftio-lib-render-{label}-{}-{n}",
            std::process::id()
        ))
    }

    /// Build a [`Config`] whose every profile shares a single library root.
    fn cfg_with_lib<I>(profiles: I, lib: &Path, post_prompt: Option<&str>) -> Config
    where
        I: IntoIterator<Item = (&'static str, Vec<&'static str>)>,
    {
        let profiles = profiles
            .into_iter()
            .map(|(name, deps)| {
                (
                    name.to_string(),
                    ProfileDef {
                        deps: deps.into_iter().map(String::from).collect(),
                        library_root: lib.to_path_buf(),
                    },
                )
            })
            .collect();
        Config {
            profiles,
            post_prompt: post_prompt.map(String::from),
        }
    }

    /// A writer that fails on the Nth `write` call, used to exercise the
    /// output-write error branches.
    struct FailAfterN {
        writes_done: usize,
        fail_on: usize,
    }

    impl Write for FailAfterN {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes_done += 1;
            if self.writes_done == self.fail_on {
                Err(io::Error::other("synthetic write failure"))
            } else {
                Ok(buf.len())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn render_json_full_includes_default_pre_system_info_and_fragments() {
        let lib = unique_temp_path("json-full");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::write(lib.join("a/x.md"), b"AX-CONTENT\n").unwrap();
        let cfg = cfg_with_lib([("root", vec!["a/x.md"])], &lib, None);

        let mut out = Vec::new();
        render_to_writer(
            &cfg,
            &mut out,
            &["root".to_string()],
            None,
            None,
            None,
            None,
            Framing::Full,
            JsonOutput::Json,
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(value["ok"], serde_json::json!(true));
        assert_eq!(value["command"], serde_json::json!("run"));
        assert_eq!(value["data"]["profile"], serde_json::json!("root"));
        assert_eq!(
            value["data"]["pre_prompt"],
            serde_json::json!(default_pre_prompt())
        );
        let system_info = value["data"]["system_info"].as_str().unwrap();
        assert!(
            system_info.contains("Today is ") && system_info.contains(" system."),
            "system_info={system_info}"
        );
        let fragments = value["data"]["fragments"].as_array().unwrap();
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0]["path"], serde_json::json!("a/x.md"));
        assert_eq!(fragments[0]["content"], serde_json::json!("AX-CONTENT\n"));

        fs::remove_dir_all(&lib).ok();
    }

    #[test]
    fn render_json_uses_explicit_pre_prompt() {
        let lib = unique_temp_path("json-explicit-pre");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::write(lib.join("a/x.md"), b"AX\n").unwrap();
        let cfg = cfg_with_lib([("root", vec!["a/x.md"])], &lib, None);

        let mut out = Vec::new();
        render_to_writer(
            &cfg,
            &mut out,
            &["root".to_string()],
            None,
            None,
            Some("EXPLICIT-PRE"),
            None,
            Framing::Full,
            JsonOutput::Json,
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(
            value["data"]["pre_prompt"],
            serde_json::json!("EXPLICIT-PRE")
        );

        fs::remove_dir_all(&lib).ok();
    }

    #[test]
    fn render_json_bare_drops_default_pre_and_system_info() {
        let lib = unique_temp_path("json-bare");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::write(lib.join("a/x.md"), b"AX\n").unwrap();
        let cfg = cfg_with_lib([("root", vec!["a/x.md"])], &lib, None);

        let mut out = Vec::new();
        render_to_writer(
            &cfg,
            &mut out,
            &["root".to_string()],
            None,
            None,
            None,
            None,
            Framing::Bare,
            JsonOutput::Json,
        )
        .unwrap();

        let text = String::from_utf8(out).unwrap();
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        // Bare JSON suppresses the auto-injected default pre-prompt and the
        // date/system stamp, but keeps the fragment payload.
        assert_eq!(value["data"]["pre_prompt"], serde_json::json!(""));
        assert_eq!(value["data"]["system_info"], serde_json::json!(""));
        assert_eq!(
            value["data"]["fragments"][0]["content"],
            serde_json::json!("AX\n")
        );

        fs::remove_dir_all(&lib).ok();
    }

    #[test]
    fn render_json_surfaces_fragment_read_error() {
        let lib = unique_temp_path("json-read-err");
        // A directory named like a fragment: resolution accepts it because the
        // path exists, but reading its contents as a file fails.
        fs::create_dir_all(lib.join("a/x.md")).unwrap();
        let cfg = cfg_with_lib([("root", vec!["a/x.md"])], &lib, None);

        let mut out = Vec::new();
        let err = render_to_writer(
            &cfg,
            &mut out,
            &["root".to_string()],
            None,
            None,
            None,
            None,
            Framing::Full,
            JsonOutput::Json,
        )
        .unwrap_err();
        match err {
            PrompterError::Io { path, .. } => assert_eq!(path, lib.join("a/x.md")),
            other => panic!("expected Io error, got {other:?}"),
        }

        fs::remove_dir_all(&lib).ok();
    }

    #[test]
    fn render_json_surfaces_writer_error() {
        let lib = unique_temp_path("json-write-err");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::write(lib.join("a/x.md"), b"AX\n").unwrap();
        let cfg = cfg_with_lib([("root", vec!["a/x.md"])], &lib, None);

        let mut w = FailAfterN {
            writes_done: 0,
            fail_on: 1,
        };
        let err = render_to_writer(
            &cfg,
            &mut w,
            &["root".to_string()],
            None,
            None,
            None,
            None,
            Framing::Full,
            JsonOutput::Json,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Write error"), "err={err}");

        fs::remove_dir_all(&lib).ok();
    }

    #[test]
    fn render_text_surfaces_fragment_read_error() {
        let lib = unique_temp_path("text-read-err");
        // A directory shaped like a fragment triggers the text-mode read error.
        fs::create_dir_all(lib.join("a/x.md")).unwrap();
        let cfg = cfg_with_lib([("root", vec!["a/x.md"])], &lib, None);

        let mut out = Vec::new();
        let err = render_to_writer(
            &cfg,
            &mut out,
            &["root".to_string()],
            None,
            None,
            None,
            None,
            Framing::Full,
            JsonOutput::Text,
        )
        .unwrap_err();
        match err {
            PrompterError::Io { path, .. } => assert_eq!(path, lib.join("a/x.md")),
            other => panic!("expected Io error, got {other:?}"),
        }

        fs::remove_dir_all(&lib).ok();
    }

    #[test]
    fn render_to_writer_propagates_resolve_error() {
        let lib = unique_temp_path("resolve-err");
        fs::create_dir_all(&lib).unwrap();
        // The profile depends on a fragment that does not exist, so resolution
        // fails and the error propagates through `render_to_writer`.
        let cfg = cfg_with_lib([("root", vec!["missing.md"])], &lib, None);

        let mut out = Vec::new();
        let err = render_to_writer(
            &cfg,
            &mut out,
            &["root".to_string()],
            None,
            None,
            None,
            None,
            Framing::Full,
            JsonOutput::Text,
        )
        .unwrap_err();
        assert!(matches!(err, PrompterError::Resolve(_)), "err={err}");

        fs::remove_dir_all(&lib).ok();
    }

    #[test]
    fn run_render_stdout_with_explicit_config_succeeds() {
        let root = unique_temp_path("stdout-cfg");
        fs::create_dir_all(root.join("library/a")).unwrap();
        fs::write(root.join("library/a/x.md"), b"AX\n").unwrap();
        let config = root.join("config.toml");
        fs::write(&config, "[root]\ndepends_on = [\"a/x.md\"]\n").unwrap();

        // Renders to stdout (captured by the test harness); a successful return
        // exercises the load-bundle-then-render stdout path.
        assert!(
            run_render_stdout(
                &["root".to_string()],
                None,
                None,
                None,
                None,
                Framing::Full,
                Some(&config),
                JsonOutput::Text,
            )
            .is_ok()
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn render_to_vec_with_explicit_config_returns_bytes() {
        let root = unique_temp_path("vec-cfg");
        fs::create_dir_all(root.join("library/a")).unwrap();
        fs::write(root.join("library/a/x.md"), b"AX-VEC\n").unwrap();
        let config = root.join("config.toml");
        fs::write(&config, "[root]\ndepends_on = [\"a/x.md\"]\n").unwrap();

        let bytes = render_to_vec(&["root".to_string()], None, Some(&config)).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            text.starts_with("You are an LLM coding agent."),
            "text={text}"
        );
        assert!(text.contains("AX-VEC\n"), "text={text}");
        assert!(
            text.ends_with(
                "Now, read the @AGENTS.md and @CLAUDE.md files in this directory, if they exist."
            ),
            "text={text}"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn render_to_vec_propagates_resolution_error() {
        let root = unique_temp_path("vec-resolve-err");
        fs::create_dir_all(root.join("library")).unwrap();
        let config = root.join("config.toml");
        fs::write(&config, "[root]\ndepends_on = [\"missing.md\"]\n").unwrap();

        let err = render_to_vec(&["root".to_string()], None, Some(&config)).unwrap_err();
        assert!(matches!(err, PrompterError::Resolve(_)), "err={err}");

        fs::remove_dir_all(&root).ok();
    }

    fn render_mode_fixture(cfg: &Config, framing: Framing, output: JsonOutput) -> Vec<u8> {
        let mut rendered = Vec::new();
        render_to_writer(
            cfg,
            &mut rendered,
            &["root".to_string()],
            None,
            None,
            None,
            None,
            framing,
            output,
        )
        .unwrap();
        rendered
    }

    fn expected_json_fixture(framing: Framing) -> serde_json::Value {
        let (pre_prompt, system_info) = match framing {
            Framing::Full => {
                let date = Local::now().format("%Y-%m-%d").to_string();
                (
                    default_pre_prompt(),
                    format!(
                        "Today is {date}, and you are running on a {}/{} system.",
                        env::consts::ARCH,
                        env::consts::OS
                    ),
                )
            }
            Framing::Bare => (String::new(), String::new()),
        };
        serde_json::json!({
            "command": "run",
            "data": {
                "fragments": [
                    {"content": "ONE", "path": "a/one.md"},
                    {"content": "TWO", "path": "a/two.md"}
                ],
                "pre_prompt": pre_prompt,
                "profile": "root",
                "system_info": system_info
            },
            "ok": true
        })
    }

    #[test]
    fn render_modes_match_pre_extraction_fixtures() {
        let lib = unique_temp_path("mode-fixtures");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::write(lib.join("a/one.md"), b"ONE").unwrap();
        fs::write(lib.join("a/two.md"), b"TWO").unwrap();
        let cfg = cfg_with_lib([("root", vec!["a/one.md", "a/two.md"])], &lib, None);

        let full_text = render_mode_fixture(&cfg, Framing::Full, JsonOutput::Text);
        let expected_full_text = format!(
            "{}\n{}\nONE\nTWO\n\n{}",
            default_pre_prompt(),
            format_system_prefix(),
            default_post_prompt()
        );
        assert_eq!(String::from_utf8(full_text).unwrap(), expected_full_text);

        let bare_text = render_mode_fixture(&cfg, Framing::Bare, JsonOutput::Text);
        assert_eq!(String::from_utf8(bare_text).unwrap(), "ONE\nTWO");

        let full_json = render_mode_fixture(&cfg, Framing::Full, JsonOutput::Json);
        let full_value: serde_json::Value = serde_json::from_slice(&full_json).unwrap();
        assert_eq!(full_value, expected_json_fixture(Framing::Full));

        let bare_json = render_mode_fixture(&cfg, Framing::Bare, JsonOutput::Json);
        let bare_value: serde_json::Value = serde_json::from_slice(&bare_json).unwrap();
        assert_eq!(bare_value, expected_json_fixture(Framing::Bare));

        fs::remove_dir_all(&lib).ok();
    }
}
