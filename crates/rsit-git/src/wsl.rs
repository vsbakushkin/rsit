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
        let mut cmd = std::process::Command::new("wsl.exe");
        cmd.args(["--distribution", &self.distro, "--exec", "true"]);
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
        let status = cmd.stdin(std::process::Stdio::null()).output()?.status;
        if !status.success() {
            return Err(std::io::Error::other(format!("cannot start WSL distribution {}", self.distro)));
        }
        Ok(())
    }

    /// The Windows path of `linux` (absolute) in the same distribution.
    pub fn host(&self, linux: &str) -> PathBuf {
        let mut path = PathBuf::from(format!(r"\\{}\{}\", self.server, self.distro));
        path.extend(linux.split('/').filter(|s| !s.is_empty()));
        path
    }
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
