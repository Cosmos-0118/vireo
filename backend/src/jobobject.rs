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

/// One process and every descendant it starts, held in a Job Object of their own (nested inside
/// the app-wide job) so they can be ended together: a terminal's shell plus the dev server,
/// agent CLI or build it launched. Closing the handle (drop) does *not* kill anything; call
/// [`ProcessTree::terminate`] for that.
#[cfg(windows)]
pub struct ProcessTree(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: a job handle is a kernel object handle; Win32 job calls are thread-safe.
#[cfg(windows)]
unsafe impl Send for ProcessTree {}
#[cfg(windows)]
unsafe impl Sync for ProcessTree {}

#[cfg(windows)]
impl ProcessTree {
    /// Puts process `pid` (and, from then on, everything it spawns) in a new job. None when the
    /// OS refuses (pre-Windows 8 without nested jobs, or the process already exited).
    pub fn adopt(pid: u32) -> Option<Self> {
        use std::mem::{size_of, zeroed};
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
        };
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

        // SAFETY: plain Win32 calls; every handle opened here is closed or owned by the result.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let tree = ProcessTree(job);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            // Same breakaway rule as the app job: a spawn that explicitly asks to leave may.
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_BREAKAWAY_OK;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                return None;
            }
            let ok = AssignProcessToJobObject(job, process);
            CloseHandle(process);
            (ok != 0).then_some(tree)
        }
    }

    /// Ends every process still in the tree.
    pub fn terminate(&self) {
        // SAFETY: `self.0` is a live job handle owned by this value.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        // SAFETY: closing the job handle this value owns, exactly once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// `CREATE_BREAKAWAY_FROM_JOB`: the spawned process leaves the kill-on-close job.
#[cfg(windows)]
pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
