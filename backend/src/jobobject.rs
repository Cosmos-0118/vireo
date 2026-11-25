//! Ties every process Vireo starts to Vireo's own lifetime (Windows).
//!
//! Terminal shells, dev servers, agent CLIs and git/rg helpers all run with the workspace as
//! their working directory, and Windows refuses to delete or rename a folder that is any
//! process's cwd. The app exits through `process::exit` (so `Drop` / `kill_on_drop` never run),
//! and a crash or "End task" skips cleanup entirely, which used to leave those children
//! orphaned and the folder locked.
//!
//! The fix lives in the OS: put this process in a Job Object flagged `KILL_ON_JOB_CLOSE`.
//! Children inherit membership, and when the last handle to the job closes (which happens when
//! this process ends for *any* reason) Windows terminates every process in it, descendants
//! included.

#[cfg(windows)]
pub fn init() {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: plain Win32 calls with a zero-initialised POD struct of the size we report. The
    // job handle is intentionally never closed: it must live exactly as long as the process.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        // BREAKAWAY_OK lets a spawn that asks for `CREATE_BREAKAWAY_FROM_JOB` (the user's
        // browser) outlive the app; everything else is killed with it.
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            return;
        }
        // Fails only if the process is in a non-nestable job (pre-Windows 8); nothing to do then.
        let _ = AssignProcessToJobObject(job, GetCurrentProcess());
    }
}

#[cfg(not(windows))]
pub fn init() {}

/// `CREATE_BREAKAWAY_FROM_JOB`: the spawned process leaves the kill-on-close job.
#[cfg(windows)]
pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
