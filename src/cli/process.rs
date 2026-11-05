use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct ProcessManager;

impl ProcessManager {
    fn pid_file_path() -> PathBuf {
        let dir = Path::new(".satsgrid");
        if !dir.exists() {
            let _ = fs::create_dir_all(dir);
        }
        dir.join("node.pid")
    }

    pub fn write_pid(pid: u32) -> std::io::Result<()> {
        fs::write(Self::pid_file_path(), pid.to_string())
    }

    pub fn read_pid() -> Option<u32> {
        let content = fs::read_to_string(Self::pid_file_path()).ok()?;
        content.trim().parse::<u32>().ok()
    }

    pub fn remove_pid() {
        let _ = fs::remove_file(Self::pid_file_path());
    }

    #[cfg(unix)]
    pub fn is_process_alive(pid: u32) -> bool {
        // Send signal 0 to check if process exists on Unix
        if let Ok(status) = Command::new("kill").arg("-0").arg(pid.to_string()).status() {
            status.success()
        } else {
            false
        }
    }

    #[cfg(windows)]
    pub fn is_process_alive(pid: u32) -> bool {
        // Use tasklist on Windows to check if PID is running
        if let Ok(output) = Command::new("tasklist")
            .arg("/FI")
            .arg(format!("PID eq {}", pid))
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            text.contains(&pid.to_string())
        } else {
            false
        }
    }

    #[cfg(not(any(unix, windows)))]
    pub fn is_process_alive(_pid: u32) -> bool {
        false
    }

    #[cfg(unix)]
    pub fn stop_process(pid: u32) -> std::io::Result<bool> {
        let status = Command::new("kill")
            .arg("-15") // SIGTERM
            .arg(pid.to_string())
            .status()?;
        Ok(status.success())
    }

    #[cfg(windows)]
    pub fn stop_process(pid: u32) -> std::io::Result<bool> {
        let status = Command::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/F")
            .status()?;
        Ok(status.success())
    }

    #[cfg(not(any(unix, windows)))]
    pub fn stop_process(_pid: u32) -> std::io::Result<bool> {
        Ok(false)
    }
}
