use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
    path::PathBuf,
    rc::Rc,
};

use anyhow::Context;

use crate::{
    channel::Channel,
    manifest::{Manifest, VersionedManifest},
    state::LocalState,
    toolchain::Toolchain,
    utils,
};

/// This struct holds contextual information about the environment in which midenup/miden will
/// operate under. This meant to be a *read-only* data structure.
#[derive(Debug)]
pub struct Config {
    /// The path to the current working directory in which midenup/miden was called from.
    pub working_directory: PathBuf,
    /// The path to the midenup's home directory, which holds all the installed toolchains with
    /// their respective libraries and executables.
    ///
    /// By default, it will point to `$XDG_DATA_HOME/midenup`; although a custom path can be
    /// specified via the `MIDENUP_HOME` environment variable, like so:
    ///
    /// `MIDENUP_HOME=/path/to/custom/home midenup`
    pub midenup_home: PathBuf,
    /// The path to `$CARGO_HOME`
    pub cargo_home: PathBuf,
    /// This represents the upstream manifest, which contains the state of all the available
    /// toolchains with their respective components.
    ///
    /// It is usually going to be obtained from `curl`ing the URI present in
    /// [`crate::manifest::VersionedManifest::PUBLISHED_MANIFEST_URI`], although it could also be
    /// obtained
    /// from a different source (be it a local file or a different URL) for debugging purposes. The
    /// source can be specified via the `MIDENUP_MANIFEST_URI` environment variable. For example:
    ///
    /// `MIDENUP_MANIFEST_URI=file://your-custom-manifest.json midenup`
    ///
    /// For more information about the Manifest's fields and format, see [Manifest].
    ///
    /// Fetched lazily, on the first operation that actually needs it. `miden <cmd>` against an
    /// installed toolchain needs nothing from upstream (spec section 13.1), and fetching
    /// unconditionally would put a network round trip in front of every component invocation.
    manifest_uri: String,
    manifest: std::cell::OnceCell<Manifest>,
    /// This flag is used to detect/distinguish when midenup is being used in tests.
    ///
    /// At the time of writing, this is mostly done to install debug builds of the various miden
    /// components to speed tests up.
    pub debug: bool,
    /// The machine's triplet (e.g. `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, etc).
    ///
    /// This is used to determine which artifact to download. If, for whatever reason (which should
    /// be rare), we fail to obtain the system's target triple, then we leave it as `None`. In
    /// those cases, we will simply install everything from source.
    pub target: Cow<'static, str>,
    /// The output of the child process executed by the `miden` CLI
    capture_output: Option<Rc<core::cell::RefCell<std::process::Output>>>,
    /// An optional input buffer that should replace the inherited standard input of the process
    pipe_stdin: Option<Vec<u8>>,
}

impl Config {
    pub fn init(
        working_directory: PathBuf,
        midenup_home: PathBuf,
        cargo_home: PathBuf,
        manifest_uri: impl AsRef<str>,
        debug: bool,
    ) -> anyhow::Result<Config> {
        let target = Cow::Borrowed(env!("TARGET"));

        Ok(Config {
            working_directory,
            midenup_home,
            cargo_home,
            manifest_uri: manifest_uri.as_ref().to_string(),
            manifest: std::cell::OnceCell::new(),
            debug,
            target,
            capture_output: None,
            pipe_stdin: None,
        })
    }

    /// Enables output capture for child processes of the `miden` CLI
    ///
    /// Returns a ref-counted cell that wraps the output captured from the child process.
    /// Callers should only attempt to access the buffer contents after `miden` has finished
    /// executing.
    pub fn capture_output(&mut self) -> Rc<core::cell::RefCell<std::process::Output>> {
        if let Some(captured) = self.capture_output.clone() {
            return captured;
        }
        let capture_output = Rc::new(core::cell::RefCell::new(std::process::Output {
            status: Default::default(),
            stderr: Default::default(),
            stdout: Default::default(),
        }));
        self.capture_output = Some(capture_output.clone());
        capture_output
    }

    /// Pipes `buffer` to child processes of `miden`, rather than inheriting the parent's stdin.
    pub fn pipe_stdin(&mut self, buffer: Vec<u8>) {
        assert!(self.pipe_stdin.replace(buffer).is_none(), "input has already been redirected");
    }

    /// The upstream manifest, fetched on first use.
    ///
    /// Only an operation that genuinely needs to know what exists upstream should call this:
    /// installing, updating, or listing what is available. Everything dispatch does -- finding the
    /// active toolchain, resolving a command, running it -- is answered by `state.json` and the
    /// active publication.
    ///
    /// A successful fetch is cached verbatim. A failed one falls back to that cache and says so:
    /// an operation that can proceed against a manifest from an hour ago is better served by doing
    /// that loudly than by failing because a network was briefly unavailable.
    pub fn upstream_manifest(&self) -> anyhow::Result<&Manifest> {
        if let Some(manifest) = self.manifest.get() {
            return Ok(manifest);
        }

        let manifest = self.fetch_upstream_manifest()?;
        Ok(self.manifest.get_or_init(|| manifest))
    }

    fn fetch_upstream_manifest(&self) -> anyhow::Result<Manifest> {
        let cache = crate::paths::manifest_cache(&self.midenup_home);

        let fetch_error = match VersionedManifest::read_from(&self.manifest_uri) {
            Ok(contents) => match VersionedManifest::parse_str(&contents) {
                Ok(manifest) => {
                    // Best effort: a manifest we could not cache is still a manifest we can use.
                    let _ = std::fs::create_dir_all(&self.midenup_home);
                    let _ = std::fs::write(&cache, &contents);
                    return Ok(manifest);
                },
                Err(err) => err,
            },
            Err(err) => err,
        };

        let cached = VersionedManifest::load_from_file(&cache).with_context(|| {
            format!("unable to fetch the toolchain manifest from '{}'", self.manifest_uri)
        });

        match cached {
            Ok(manifest) => {
                crate::warn!(
                    "could not reach '{}' ({fetch_error}); using the cached manifest from '{}', \
                     which may be out of date",
                    self.manifest_uri,
                    cache.display(),
                );
                Ok(manifest)
            },
            // Report the *fetch* failure: it is the one the user can act on. The absent cache is a
            // consequence of never having fetched successfully, not an independent problem.
            Err(_) => Err(anyhow::Error::new(fetch_error).context(format!(
                "unable to fetch the toolchain manifest from '{}', and no cached copy is available",
                self.manifest_uri
            ))),
        }
    }

    #[inline]
    pub fn target(&self) -> &str {
        self.target.as_ref()
    }

    /// Where local installation state lives.
    pub fn state_path(&self) -> PathBuf {
        crate::paths::state_path(&self.midenup_home)
    }

    /// Reads what this machine has installed.
    pub fn local_state(&self) -> anyhow::Result<LocalState> {
        LocalState::load(&self.state_path()).context("unable to load local state")
    }

    /// Writes local installation state, refusing to commit anything that cannot be read back.
    pub fn write_local_state(&self, state: &LocalState) -> anyhow::Result<()> {
        state.save(&self.state_path()).context("unable to write local state")
    }

    /// Points `$MIDENUP_HOME/opt` at the active toolchain's shims.
    ///
    /// Runs after every command, including `miden` dispatch, so it resolves the active channel from
    /// *local* state: asking upstream which channel `mainnet` names would put a network round trip
    /// after every component invocation, which is exactly what section 13.1 forbids.
    pub fn update_opt_symlinks(&self) -> anyhow::Result<()> {
        let (current_toolchain, _) = Toolchain::current(self, None)?;

        // Directory which point to the directory where symlinks are stored
        let opt_dir = self.midenup_home.join("opt");

        let Some(active_channel) = self.local_channel(&current_toolchain.channel) else {
            // Nothing installed for it, so there is nothing to point at. Not an error: `midenup
            // install` runs this on the way to installing exactly that.
            return Ok(());
        };
        let toolchain_dir = crate::paths::toolchain_link(&self.midenup_home, &active_channel);

        // If the currently active channel doesn't exist, then there's nothing to update regarding
        // the opt/ symlink.
        if !toolchain_dir.exists() {
            // However, if the opt directory still exists, then we remove it in order to avoid a
            // "dangling symlink". This can happen when an uninstall is issued.
            if std::fs::read_link(&opt_dir).is_ok() {
                std::fs::remove_file(&opt_dir).context("Couldn't remove 'opt' symlink")?;
            }
            return Ok(());
        }

        let update = if let Ok(pointing) = std::fs::read_link(&opt_dir) {
            // If it does exist, update it if it's pointing to a non-active toolchain.
            pointing
                .file_name()
                .and_then(|toolchain_name| toolchain_name.to_str())
                .is_some_and(|toolchain_name| toolchain_name != active_channel.to_string())
        } else {
            // If the symlink doesn't exist, update it by creating it.
            true
        };

        if update {
            // Atomically, because this runs at the end of *every* command, including ones that
            // take no lock: two `miden` invocations would otherwise race to create it and one
            // would fail with `EEXIST`.
            let opt_path = toolchain_dir.join("opt");
            utils::fs::replace_symlink(&opt_dir, &opt_path).with_context(|| {
                format!(
                    "Failed to create opt/ symlink from {} to {}",
                    opt_dir.display(),
                    opt_path.display()
                )
            })?;
        }

        Ok(())
    }

    /// Resolves a user-facing channel name against what is *installed*, without upstream.
    ///
    /// Which channel a network names is a property of the upstream manifest, but the
    /// `toolchains/<network>` symlink records the last answer upstream gave that this machine acted
    /// on, so dispatch can name the active channel offline.
    pub fn local_channel(&self, channel: &crate::channel::UserChannel) -> Option<semver::Version> {
        use crate::channel::UserChannel;

        match channel {
            UserChannel::Version(version) => Some(version.clone()),
            // The `toolchains/<network>` symlink records the last answer upstream gave that this
            // machine acted on. There is deliberately no fallback: "the highest installed version"
            // is a plausible wrong answer for mainnet, and an unresolvable network should send the
            // caller upstream, which install and update consult anyway.
            UserChannel::Named(name) => {
                std::fs::read_link(crate::paths::network_link(&self.midenup_home, name.as_ref()))
                    .ok()
                    .and_then(|target| {
                        target
                            .file_name()
                            .and_then(|name| name.to_str())
                            .and_then(|name| semver::Version::parse(name).ok())
                    })
            },
        }
    }

    pub fn toolchain_dir(&self, channel: &Channel) -> PathBuf {
        crate::paths::toolchain_link(&self.midenup_home, &channel.name)
    }

    /// Executes a command.
    pub fn execute_command(
        &self,
        active_toolchain: &Channel,
        target_exe: &OsStr,
        args: &[OsString],
    ) -> Result<std::process::ExitStatus, std::io::Error> {
        let toolchain_name = active_toolchain.name.to_string();
        let sysroot = self.midenup_home.join("toolchains").join(&toolchain_name);
        let toolchain_opt = sysroot.join("opt");

        // Get the current PATH, and override CARGO_HOME if it differs from the inherited CARGO_HOME
        let (cargo_home, path) = match std::env::var_os("CARGO_HOME") {
            Some(inherited) if inherited.as_os_str() == self.cargo_home.as_os_str() => {
                (inherited, std::env::var_os("PATH"))
            },
            Some(_) => match std::env::var_os("PATH") {
                Some(prev_path) => {
                    let mut path =
                        OsString::from(format!("{}:", self.cargo_home.join("bin").display()));
                    path.push(prev_path);
                    (self.cargo_home.clone().into_os_string(), Some(path))
                },
                None => {
                    let cargo_home = self.cargo_home.clone().into_os_string();
                    let path = self.cargo_home.join("bin").into_os_string();
                    (cargo_home, Some(path))
                },
            },
            None => (self.cargo_home.clone().into_os_string(), std::env::var_os("PATH")),
        };

        // Prepend the toolchain opt/ directory to the current PATH
        let path = match path {
            Some(prev_path) => {
                let mut path = OsString::from(format!("{}:", toolchain_opt.display()));
                path.push(prev_path);
                path
            },
            None => toolchain_opt.into_os_string(),
        };

        let mut command = std::process::Command::new(target_exe);
        command
            .env("MIDENUP_HOME", &self.midenup_home)
            .env("MIDENUP_TOOLCHAIN", &toolchain_name)
            .env("MIDEN_SYSROOT", &sysroot)
            .env("CARGO_HOME", cargo_home)
            .env("PATH", path)
            .args(args);

        if self.pipe_stdin.is_some() {
            command.stdin(std::process::Stdio::piped());
        }

        if self.capture_output.is_some() {
            command
                .stderr(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped());
        } else {
            command
                .stderr(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit());
        }

        let mut child = command.spawn()?;

        if let Some(bytes) = self.pipe_stdin.clone() {
            let mut stdin = child.stdin.take().expect("failed to open stdin");
            std::thread::spawn(move || {
                use std::io::Write;

                stdin.write_all(&bytes).expect("failed to write to stdin");
            });
        }

        if let Some(capture_output) = self.capture_output.as_deref() {
            let std::process::Output { status, stderr, stdout } = child.wait_with_output()?;
            let mut capture_output = capture_output.borrow_mut();
            capture_output.status = status;
            capture_output.stderr = stderr;
            capture_output.stdout = stdout;
            Ok(status)
        } else {
            child.wait()
        }
    }
}
