//! Windows Job Object:子程序(llama-server、bridge)跟著本程式一起結束。
//! 本程式被工作管理員強制結束時,job handle 關閉,系統會殺掉 job 裡所有程序。

use std::io;
use std::process::{Child, Command};

pub struct Job {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

// HANDLE 只在建立與 assign 時使用,Job 本身不會跨執行緒同時操作同一個 handle 以外的狀態
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    /// 建立帶 JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE 的 job
    #[cfg(windows)]
    pub fn new() -> io::Result<Self> {
        use windows::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY:參數都是有效值;回傳的 handle 由 Job 持有到程式結束
        unsafe {
            let handle = CreateJobObjectW(None, None).map_err(io::Error::other)?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .map_err(io::Error::other)?;
            Ok(Self { handle })
        }
    }

    #[cfg(windows)]
    pub fn assign(&self, child: &Child) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        // SAFETY:child 的 handle 在 child 存活期間有效
        unsafe { AssignProcessToJobObject(self.handle, HANDLE(child.as_raw_handle())).map_err(io::Error::other) }
    }

    #[cfg(not(windows))]
    pub fn new() -> io::Result<Self> {
        Ok(Self {})
    }

    #[cfg(not(windows))]
    pub fn assign(&self, _child: &Child) -> io::Result<()> {
        Ok(())
    }
}

/// 子程序不要跳出主控台視窗
pub fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// pid 是否還活著(native host 判斷本程式在不在)
#[cfg(windows)]
pub fn pid_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY:OpenProcess 失敗回 Err;成功的 handle 用完即關
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code).is_ok();
        let _ = CloseHandle(h);
        ok && code == STILL_ACTIVE.0 as u32
    }
}

#[cfg(not(windows))]
pub fn pid_alive(_pid: u32) -> bool {
    false
}
