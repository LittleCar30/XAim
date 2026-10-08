use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use tokio::process::Child;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

pub struct ProcessJob(OwnedHandle);

impl ProcessJob {
    pub fn attach(child: &Child) -> io::Result<Self> {
        // The owned job kills all inherited descendants when its last handle closes.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(OwnedHandle::from_raw_handle(handle));
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let process = child
                .raw_handle()
                .ok_or_else(|| io::Error::other("Child process handle unavailable"))?;
            if AssignProcessToJobObject(job.0.as_raw_handle(), process) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }
}
