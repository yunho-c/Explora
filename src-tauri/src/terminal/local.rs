use std::{
    io::{Read, Write},
    path::Path,
};

use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

use crate::filesystem::ExplorerError;

pub struct SpawnedLocalPty {
    pub master: Box<dyn MasterPty + Send>,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
}

pub fn spawn_default_shell(
    working_directory: &Path,
    size: PtySize,
) -> Result<SpawnedLocalPty, ExplorerError> {
    spawn_command(default_shell_command(working_directory), size)
}

pub(super) fn spawn_command(
    command: CommandBuilder,
    size: PtySize,
) -> Result<SpawnedLocalPty, ExplorerError> {
    let pair = native_pty_system()
        .openpty(size)
        .map_err(|_| terminal_io_error("Explora could not create a local terminal."))?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|_| terminal_io_error("Explora could not open terminal output."))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|_| terminal_io_error("Explora could not open terminal input."))?;

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|_| terminal_io_error("Explora could not start the default shell."))?;
    let killer = child.clone_killer();
    drop(pair.slave);

    Ok(SpawnedLocalPty {
        master: pair.master,
        reader,
        writer,
        child,
        killer,
    })
}

fn default_shell_command(working_directory: &Path) -> CommandBuilder {
    let mut command = CommandBuilder::new_default_prog();
    command.cwd(working_directory.as_os_str());
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "Explora");
    command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));

    // These names are reserved for in-memory broker work. They are removed
    // defensively even though Explora does not currently place secrets in its
    // own process environment.
    for key in [
        "EXPLORA_SSH_PASSWORD",
        "EXPLORA_SSH_PASSPHRASE",
        "EXPLORA_SSH_PROMPT_RESPONSE",
    ] {
        command.env_remove(key);
    }
    command
}

fn terminal_io_error(message: &str) -> ExplorerError {
    ExplorerError::Io {
        message: message.to_owned(),
        kind: std::io::ErrorKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Read},
        path::Path,
        process::{Command, Stdio},
        thread,
        time::Duration,
    };

    use portable_pty::{CommandBuilder, PtySize};

    use super::{default_shell_command, spawn_command};

    #[test]
    fn shell_environment_is_product_owned_and_secret_free() {
        let command = default_shell_command(Path::new("/"));
        assert_eq!(command.get_env("TERM"), Some("xterm-256color".as_ref()));
        assert_eq!(command.get_env("COLORTERM"), Some("truecolor".as_ref()));
        assert_eq!(command.get_env("TERM_PROGRAM"), Some("Explora".as_ref()));
        assert_eq!(command.get_env("EXPLORA_SSH_PASSWORD"), None);
        assert_eq!(
            command.get_cwd().map(|path| path.as_os_str()),
            Some("/".as_ref())
        );
    }

    #[cfg(unix)]
    #[test]
    fn real_pty_preserves_bytes_resizes_and_reports_exit() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "printf '\\001\\177\\377terminal-fixture\\n'; exit 7"]);
        let mut transport = spawn_command(
            command,
            PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
        .expect("spawn fixture PTY");

        transport
            .master
            .resize(PtySize {
                rows: 41,
                cols: 113,
                pixel_width: 900,
                pixel_height: 600,
            })
            .expect("resize PTY");
        let size = transport.master.get_size().expect("read PTY size");
        assert_eq!((size.rows, size.cols), (41, 113));

        let mut bytes = Vec::new();
        transport
            .reader
            .read_to_end(&mut bytes)
            .expect("read fixture output");
        let status = transport.child.wait().expect("wait for fixture");

        assert!(bytes
            .windows(b"terminal-fixture".len())
            .any(|part| part == b"terminal-fixture"));
        assert!(bytes.contains(&1));
        assert!(bytes.contains(&127));
        assert!(bytes.contains(&255));
        assert_eq!(status.exit_code(), 7);
    }

    #[cfg(windows)]
    #[test]
    fn conpty_fixture_resizes_reports_output_and_exit() {
        let mut command = CommandBuilder::new("cmd.exe");
        command.args(["/D", "/S", "/C", "<nul set /p =terminal-fixture& exit /b 7"]);
        let mut transport = spawn_command(
            command,
            PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
        .expect("spawn ConPTY fixture");

        transport
            .master
            .resize(PtySize {
                rows: 41,
                cols: 113,
                pixel_width: 900,
                pixel_height: 600,
            })
            .expect("resize ConPTY");
        let size = transport.master.get_size().expect("read ConPTY size");
        assert_eq!((size.rows, size.cols), (41, 113));

        let mut bytes = Vec::new();
        transport
            .reader
            .read_to_end(&mut bytes)
            .expect("read ConPTY fixture output");
        let status = transport.child.wait().expect("wait for ConPTY fixture");

        assert!(bytes
            .windows(b"terminal-fixture".len())
            .any(|part| part == b"terminal-fixture"));
        assert_eq!(status.exit_code(), 7);
    }

    #[cfg(unix)]
    #[test]
    fn killing_the_owned_pty_reaps_its_background_process_group() {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "sleep 30 & child=$!; printf '%s\\n' \"$child\"; wait"]);
        let transport = spawn_command(command, PtySize::default()).expect("spawn fixture PTY");
        let mut reader = BufReader::new(transport.reader);
        let mut child_pid = String::new();
        reader.read_line(&mut child_pid).expect("read child pid");
        let child_pid = child_pid.trim().parse::<u32>().expect("numeric child pid");

        let mut killer = transport.killer;
        killer.kill().expect("kill PTY process group");
        let mut child = transport.child;
        child.wait().expect("reap fixture shell");

        let disappeared = (0..50).any(|_| {
            let gone = !Command::new("kill")
                .args(["-0", &child_pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !gone {
                thread::sleep(Duration::from_millis(20));
            }
            gone
        });
        assert!(disappeared, "background child survived PTY shutdown");
    }
}
