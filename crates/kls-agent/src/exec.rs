//! The allowlisted host-command executor.
//!
//! [`HostCommand`] can only ever spawn a binary named by [`Tool`]; there is no
//! constructor that takes an arbitrary program name. Arguments are always passed
//! as an argv array — never concatenated into a shell string — so a value placed
//! in an argument is data, not code. The single deliberate exception is
//! [`shell`], the named escape hatch for operator-configured command strings.

use std::process::{Output, Stdio};

/// The fixed set of host binaries kls-agent is permitted to spawn. This enum is
/// the security boundary: it is closed at compile time, so no caller can cause
/// an arbitrary program to run through the typed path.
///
/// The OS resolves each name from `PATH`. `Sudo`/`Doas` are elevation helpers —
/// they may only appear as a prefix in front of another (non-elevation) tool
/// (enforced by [`HostCommand::from_argv`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// nftables (`nft`).
    Nft,
    /// Uncomplicated Firewall (`ufw`).
    Ufw,
    /// Legacy `iptables`.
    Iptables,
    /// The Docker CLI.
    Docker,
    /// `df` — filesystem usage.
    Df,
    /// `sudo` elevation prefix.
    Sudo,
    /// `doas` elevation prefix.
    Doas,
}

impl Tool {
    /// The binary name spawned for this tool.
    pub fn binary(self) -> &'static str {
        match self {
            Tool::Nft => "nft",
            Tool::Ufw => "ufw",
            Tool::Iptables => "iptables",
            Tool::Docker => "docker",
            Tool::Df => "df",
            Tool::Sudo => "sudo",
            Tool::Doas => "doas",
        }
    }

    /// Parses a binary name back into a [`Tool`], or `None` if it is not on the
    /// allowlist.
    pub fn parse(name: &str) -> Option<Tool> {
        match name {
            "nft" => Some(Tool::Nft),
            "ufw" => Some(Tool::Ufw),
            "iptables" => Some(Tool::Iptables),
            "docker" => Some(Tool::Docker),
            "df" => Some(Tool::Df),
            "sudo" => Some(Tool::Sudo),
            "doas" => Some(Tool::Doas),
            _ => None,
        }
    }

    /// Whether this tool is an elevation prefix (`sudo`/`doas`) rather than a
    /// host command in its own right.
    pub fn is_elevation(self) -> bool {
        matches!(self, Tool::Sudo | Tool::Doas)
    }
}

/// Why a host command could not be built or run.
#[derive(Debug)]
pub enum ExecError {
    /// An empty argv was supplied.
    Empty,
    /// The requested program is not on the [`Tool`] allowlist (the string is the
    /// offending program name).
    NotAllowed(String),
    /// The spawn itself failed (binary missing, permission denied, timeout, …).
    Io(std::io::Error),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Empty => write!(f, "empty command"),
            ExecError::NotAllowed(prog) => write!(f, "program `{prog}` is not on the host-command allowlist"),
            ExecError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ExecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ExecError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ExecError> for std::io::Error {
    fn from(e: ExecError) -> Self {
        match e {
            ExecError::Io(io) => io,
            other => std::io::Error::new(std::io::ErrorKind::PermissionDenied, other.to_string()),
        }
    }
}

/// A validated, allowlisted host command. The program is one of [`Tool`]'s
/// binaries (optionally an elevation prefix in front of one); arguments are an
/// argv array.
#[derive(Debug, Clone)]
pub struct HostCommand {
    program: String,
    args: Vec<String>,
    cwd: Option<String>,
}

impl HostCommand {
    /// Start a command for an allowlisted [`Tool`].
    pub fn new(tool: Tool) -> Self {
        HostCommand {
            program: tool.binary().to_string(),
            args: Vec::new(),
            cwd: None,
        }
    }

    /// Build a command from a pre-assembled argv, validating that the program
    /// (after an optional `sudo`/`doas` prefix and its flags) is an allowlisted,
    /// non-elevation [`Tool`]. Used by callers that already assemble a full argv
    /// (e.g. the firewall backend's per-rule command builders).
    pub fn from_argv(argv: &[String]) -> Result<HostCommand, ExecError> {
        let program = argv.first().ok_or(ExecError::Empty)?;
        let tool = Tool::parse(program).ok_or_else(|| ExecError::NotAllowed(program.clone()))?;
        if tool.is_elevation() {
            // The elevated target must itself be an allowlisted, non-elevation
            // tool — skip the elevation program's own flags (e.g. `-n`).
            let ok = argv[1..]
                .iter()
                .find(|t| !t.starts_with('-'))
                .and_then(|t| Tool::parse(t))
                .is_some_and(|t| !t.is_elevation());
            if !ok {
                return Err(ExecError::NotAllowed(program.clone()));
            }
        }
        Ok(HostCommand {
            program: program.clone(),
            args: argv[1..].to_vec(),
            cwd: None,
        })
    }

    /// Append a single argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Run in a specific working directory (e.g. `docker compose` in a service's
    /// project directory).
    pub fn current_dir(mut self, dir: impl Into<String>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// The program that will be spawned (for audit/preview rendering).
    pub fn program(&self) -> &str {
        &self.program
    }

    fn tokio_command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }
        cmd
    }

    /// Run to completion, capturing stdout/stderr (async).
    pub async fn output(&self) -> Result<Output, ExecError> {
        self.tokio_command().output().await.map_err(ExecError::Io)
    }

    /// Run to completion, capturing stdout/stderr (blocking). For the handful of
    /// synchronous call sites; prefer [`HostCommand::output`] on async paths.
    pub fn output_blocking(&self) -> Result<Output, ExecError> {
        let mut cmd = std::process::Command::new(&self.program);
        cmd.args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }
        cmd.output().map_err(ExecError::Io)
    }

    /// Spawn with stdout/stderr piped for streaming (e.g. `docker logs
    /// --follow`). The child is killed if the returned handle is dropped.
    pub fn spawn_piped(&self) -> Result<tokio::process::Child, ExecError> {
        let mut cmd = self.tokio_command();
        cmd.kill_on_drop(true);
        cmd.spawn().map_err(ExecError::Io)
    }
}

/// **DANGER — the one unconstrained path.** Builds a shell invocation of an
/// arbitrary `command` string (`sh -c` on Unix, `cmd /C` on Windows). This is
/// *only* for operator-configured command strings (the proxy reload command)
/// and admin cron scripts — both arbitrary by nature and gated behind admin auth
/// + audit at the call site. Everything else must use the typed [`Tool`] path.
///
/// On Unix the standard system `bin` directories are appended to `PATH` so
/// commands resolve even when the process was launched (e.g. by systemd) with a
/// stripped environment.
#[cfg(unix)]
pub fn shell(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    let path = match std::env::var_os("PATH") {
        Some(p) if !p.is_empty() => {
            let mut combined = p.into_string().unwrap_or_default();
            combined.push_str(":/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
            combined
        }
        _ => "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
    };
    cmd.env("PATH", path);
    cmd
}

/// See the Unix variant — this builds `cmd /C <command>` on Windows.
#[cfg(not(unix))]
pub fn shell(command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("cmd");
    cmd.args(["/C", command]);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tool_names_round_trip() {
        for tool in [
            Tool::Nft,
            Tool::Ufw,
            Tool::Iptables,
            Tool::Docker,
            Tool::Df,
            Tool::Sudo,
            Tool::Doas,
        ] {
            assert_eq!(Tool::parse(tool.binary()), Some(tool));
        }
        assert_eq!(Tool::parse("rm"), None);
        assert_eq!(Tool::parse("bash"), None);
    }

    #[test]
    fn from_argv_accepts_a_bare_allowlisted_tool() {
        let cmd = HostCommand::from_argv(&argv(&["nft", "list", "tables"])).unwrap();
        assert_eq!(cmd.program(), "nft");
        assert_eq!(cmd.args, argv(&["list", "tables"]));
    }

    #[test]
    fn from_argv_accepts_sudo_prefixing_an_allowlisted_tool() {
        let cmd = HostCommand::from_argv(&argv(&["sudo", "-n", "nft", "add", "rule"])).unwrap();
        assert_eq!(cmd.program(), "sudo");
        assert_eq!(cmd.args, argv(&["-n", "nft", "add", "rule"]));
    }

    #[test]
    fn from_argv_rejects_an_arbitrary_program() {
        let err = HostCommand::from_argv(&argv(&["rm", "-rf", "/"])).unwrap_err();
        assert!(matches!(err, ExecError::NotAllowed(p) if p == "rm"));
    }

    #[test]
    fn from_argv_rejects_sudo_wrapping_an_arbitrary_program() {
        // Elevation must not become a way to run anything off the allowlist.
        let err = HostCommand::from_argv(&argv(&["sudo", "-n", "rm", "-rf", "/"])).unwrap_err();
        assert!(matches!(err, ExecError::NotAllowed(_)));
    }

    #[test]
    fn from_argv_rejects_sudo_with_no_target_tool() {
        let err = HostCommand::from_argv(&argv(&["sudo", "-n"])).unwrap_err();
        assert!(matches!(err, ExecError::NotAllowed(_)));
    }

    #[test]
    fn from_argv_rejects_empty() {
        assert!(matches!(HostCommand::from_argv(&[]).unwrap_err(), ExecError::Empty));
    }

    #[test]
    fn shell_builds_the_platform_interpreter() {
        let cmd = shell("echo hi");
        let program = cmd.as_std().get_program().to_string_lossy().into_owned();
        if cfg!(unix) {
            assert_eq!(program, "sh");
        } else {
            assert_eq!(program, "cmd");
        }
    }
}
