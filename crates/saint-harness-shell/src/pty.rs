use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::Read;
use std::path::Path;

/// Executes an interactive command inside a `portable-pty` pseudo-terminal and collects its output.
pub fn run_in_pty(command: &str, cwd: &Path) -> Result<(i32, String), String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty failed: {e}"))?;

    #[cfg(unix)]
    let mut cmd = {
        let mut c = CommandBuilder::new("sh");
        c.arg("-c");
        c.arg(command);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = CommandBuilder::new("cmd.exe");
        c.arg("/C");
        c.arg(command);
        c
    };
    cmd.cwd(cwd);

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("pty spawn failed: {e}"))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("pty reader failed: {e}"))?;
    let mut output = String::new();
    let _ = reader.read_to_string(&mut output);

    let status = child.wait().map_err(|e| format!("pty wait failed: {e}"))?;
    let code = if status.success() { 0 } else { 1 };
    Ok((code, output))
}
