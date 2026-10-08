//! Process lifetime guard: cancellation must terminate descendants as well as the launcher.

use crate::errors::{Result, XaimError};
use tokio::process::{Child, Command};

pub fn configure(command: &mut Command) {
    command.env_clear();
    for key in [
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "TEMP",
        "TMP",
        "TMPDIR",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "GOPATH",
        "JAVA_HOME",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "SystemDrive",
        "INCLUDE",
        "LIB",
        "LIBPATH",
        "VCINSTALLDIR",
        "VSINSTALLDIR",
        "VisualStudioVersion",
        "VCToolsInstallDir",
        "WindowsSdkDir",
        "WindowsSDKVersion",
        "UniversalCRTSdkDir",
        "UCRTVersion",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.env("CI", "1").env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
    }
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

#[cfg(windows)]
pub struct ProcessTree(isize);

#[cfg(windows)]
impl ProcessTree {
    pub fn attach(child: &Child) -> Result<Self> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::*;
        let process = child
            .raw_handle()
            .ok_or_else(|| XaimError::Verify("验证进程句柄不可用".into()))?;
        // This handle is uniquely owned and always closed by the guard, including future cancellation.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(job, process) == 0
            {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(error.into());
            }
            Ok(Self(job as isize))
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as _);
        }
    }
}

#[cfg(unix)]
pub struct ProcessTree(u32);
#[cfg(unix)]
impl ProcessTree {
    pub fn attach(child: &Child) -> Result<Self> {
        Ok(Self(child.id().ok_or_else(|| {
            XaimError::Verify("验证进程 ID 不可用".into())
        })?))
    }
}
#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", self.0)])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(not(any(unix, windows)))]
pub struct ProcessTree;
#[cfg(not(any(unix, windows)))]
impl ProcessTree {
    pub fn attach(_child: &Child) -> Result<Self> {
        Ok(Self)
    }
}
