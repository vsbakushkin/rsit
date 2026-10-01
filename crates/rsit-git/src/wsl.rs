//! Repositories inside WSL, opened from Windows as `\\wsl.localhost\<distro>\...`
//! (or the older `\\wsl$\<distro>\...`).
//!
//! gix reads them through the share like any other directory, but `git` must
//! run inside the distribution: Windows' `git.exe` rejects them as owned by
//! someone else, sees CRLF and file mode noise, and has none of the user's
//! hooks, ssh keys or credential helpers there.

use std::path::{Component, Path, PathBuf, Prefix};

/// A path on a WSL share, split into the distribution and the Linux path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WslPath {
    /// `wsl.localhost` or `wsl$`, kept to build paths back the way they came.
    pub server: String,
    pub distro: String,
    /// Absolute Linux path, e.g. `/home/me/repo`.
    pub linux: String,
}

impl WslPath {
    /// Splits `path` if it lies on a WSL share; always `None` outside Windows.
    pub fn parse(path: &Path) -> Option<Self> {
        let mut components = path.components();
        let (server, share) = match components.next()? {
            Component::Prefix(p) => match p.kind() {
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => (server, share),
                _ => return None,
            },
            _ => return None,
        };
        let server = server.to_str()?;
        if !server.eq_ignore_ascii_case("wsl.localhost") && !server.eq_ignore_ascii_case("wsl$") {
            return None;
        }
        let mut linux = String::new();
        for c in components {
            if let Component::Normal(name) = c {
                linux.push('/');
                linux.push_str(name.to_str()?);
            }
        }
        if linux.is_empty() {
            linux.push('/');
        }
        Some(Self { server: server.to_string(), distro: share.to_str()?.to_string(), linux })
    }

    /// Starts the distribution: a stopped one (idle, or after a reboot) does not
    /// answer on its share until something runs in it.
    pub fn wake(&self) -> std::io::Result<()> {
        let status = wsl_exe(&["--distribution", &self.distro, "--exec", "true"]).output()?.status;
        if !status.success() {
            return Err(std::io::Error::other(format!("cannot start WSL distribution {}", self.distro)));
        }
        Ok(())
    }

    /// Whether the distribution is running, asked from `wsl.exe` in ~25 ms:
    /// touching the share of a stopped one instead blocks for ~20 s, then
    /// fails (and starts it).
    pub fn is_running(&self) -> bool {
        let Ok(out) = wsl_exe(&["--list", "--running", "--quiet"]).output() else { return false };
        // UTF-16 unless WSL_UTF8=1 is set
        let text = if out.stdout.get(1) == Some(&0) {
            let units: Vec<u16> = out.stdout.as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        text.lines().any(|name| name.trim().eq_ignore_ascii_case(&self.distro))
    }

    /// The Windows path of `linux` (absolute) in the same distribution.
    pub fn host(&self, linux: &str) -> PathBuf {
        let mut path = PathBuf::from(format!(r"\\{}\{}\", self.server, self.distro));
        path.extend(linux.split('/').filter(|s| !s.is_empty()));
        path
    }
}

fn wsl_exe(args: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new("wsl.exe");
    cmd.args(args).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn parses_wsl_shares() {
        let p = WslPath::parse(Path::new(r"\\wsl.localhost\Ubuntu-24.04\home\me\repo")).unwrap();
        assert_eq!(
            (p.server.as_str(), p.distro.as_str(), p.linux.as_str()),
            ("wsl.localhost", "Ubuntu-24.04", "/home/me/repo")
        );
        assert_eq!(p.host("/tmp/x"), PathBuf::from(r"\\wsl.localhost\Ubuntu-24.04\tmp\x"));
        let p = WslPath::parse(Path::new(r"\\?\UNC\WSL$\Debian\")).unwrap();
        assert_eq!((p.distro.as_str(), p.linux.as_str()), ("Debian", "/"));
        assert_eq!(WslPath::parse(Path::new(r"\\server\share\repo")), None);
        assert_eq!(WslPath::parse(Path::new(r"C:\repo")), None);
    }
}
