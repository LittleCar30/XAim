//! Process lifetime guard: cancellation must terminate descendants as well as the launcher.

use crate::errors::{Result, XaimError};
use tokio::process::{Child, Command};

/// Windows children must join their job before any user code can execute. This
/// also prevents descendants escaping the job and fast exits racing assignment.
pub fn spawn(command: &mut Command) -> std::io::Result<(Child, ProcessTree)> {
    #[cfg(windows)]
    command.creation_flags(0x08000000 | 0x00000004); // NO_WINDOW | SUSPENDED
    let mut child = command.spawn()?;
    let tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.start_kill();
            return Err(std::io::Error::other(format!(
                "绑定命令进程管理对象失败: {error}"
            )));
        }
    };
    #[cfg(windows)]
    if let Err(error) = resume(child.id().unwrap()) {
        let _ = child.start_kill();
        return Err(error);
    }
    Ok((child, tree))
}

#[cfg(windows)]
fn resume(pid: u32) -> std::io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    // CREATE_SUSPENDED leaves exactly the initial thread suspended. The child
    // cannot exit/reuse its PID while we locate and resume that thread.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::other(format!(
                "读取挂起进程线程失败: {}",
                std::io::Error::last_os_error()
            )));
        }
        let snapshot = OwnedHandle::from_raw_handle(snapshot);
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut found = Thread32First(snapshot.as_raw_handle(), &mut entry);
        while found != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    return Err(std::io::Error::other(format!(
                        "打开命令初始线程失败: {}",
                        std::io::Error::last_os_error()
                    )));
                }
                let thread = OwnedHandle::from_raw_handle(thread);
                if ResumeThread(thread.as_raw_handle()) == u32::MAX {
                    return Err(std::io::Error::other(format!(
                        "恢复命令初始线程失败: {}",
                        std::io::Error::last_os_error()
                    )));
                }
                return Ok(());
            }
            found = Thread32Next(snapshot.as_raw_handle(), &mut entry);
        }
        Err(std::io::Error::other("找不到命令进程的初始线程"))
    }
}

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

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;

    #[tokio::test]
    async fn short_commands_start_inside_the_job_before_they_can_exit() {
        // The previous spawn-then-attach sequence deterministically rejects an
        // already-exited process with ERROR_ACCESS_DENIED.
        let mut old = Command::new("cmd");
        configure(&mut old);
        old.args(["/C", "exit", "0"]);
        let mut exited = old.spawn().unwrap();
        assert!(exited.wait().await.unwrap().success());
        assert!(ProcessTree::attach(&exited).is_err());
        for _ in 0..20 {
            let mut command = Command::new("cmd");
            configure(&mut command);
            command.args(["/C", "exit", "0"]).kill_on_drop(true);
            let (mut child, tree) = spawn(&mut command).unwrap();
            let mut in_job = 0;
            assert_ne!(
                unsafe { IsProcessInJob(child.raw_handle().unwrap(), tree.0 as _, &mut in_job) },
                0
            );
            assert_ne!(in_job, 0);
            // Delayed observation cannot turn a successful short command into
            // a failure to attach the Windows job.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            assert!(child.wait().await.unwrap().success());
        }
    }

    #[tokio::test]
    async fn closing_job_stops_descendants_even_after_launcher_exits() {
        let root = tempfile::tempdir().unwrap();
        let mut command = Command::new(crate::verify::runner::resolve_program("node"));
        configure(&mut command);
        command.current_dir(root.path()).args(["-e",
            "require('child_process').spawn(process.execPath,['-e',\"setTimeout(()=>require('fs').writeFileSync('leaked','bad'),1000)\"],{stdio:'ignore'}).unref()"])
            .kill_on_drop(true);
        let (mut child, tree) = spawn(&mut command).unwrap();
        assert!(child.wait().await.unwrap().success());
        drop(tree);
        tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
        assert!(!root.path().join("leaked").exists());
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
