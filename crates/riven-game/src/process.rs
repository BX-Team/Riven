/// When the OS says process `pid` started, or `None` when it is gone; a reused pid gets a new value.
pub fn started(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // `comm` may hold spaces and parentheses, so fields are counted after the last `)`.
        let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace();
        if fields.next()? == "Z" {
            return None;
        }
        fields.nth(18)?.parse().ok()
    }
    #[cfg(target_os = "macos")]
    {
        let pid = libc::c_int::try_from(pid).ok()?;
        // SAFETY: proc_bsdinfo is plain data; the kernel fills at most `size` bytes of it.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let read = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        (read == size && info.pbi_status != libc::SZOMB)
            .then(|| info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let empty = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut times = [empty; 4];
        let mut code = 0u32;
        // SAFETY: the handle is checked and closed; the out pointers live on this stack.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let [created, exited, kernel, user] = &mut times;
            let alive = GetExitCodeProcess(handle, &mut code) != 0
                && code == STILL_ACTIVE as u32
                && GetProcessTimes(handle, created, exited, kernel, user) != 0;
            CloseHandle(handle);
            alive.then(|| {
                (u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime)
            })
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let pid = libc::pid_t::try_from(pid).ok()?;
        // SAFETY: signal 0 only checks that the process exists.
        let exists = unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        exists.then_some(0)
    }
}

/// Whether process `pid` still runs and, when `started` is known, is the one that started then.
pub fn alive(pid: u32, started: Option<u64>) -> bool {
    self::started(pid).is_some_and(|now| started.is_none_or(|then| then == now))
}

/// Asks a game to close: SIGTERM lets it save; Windows has no such signal, so it is ended.
pub fn kill(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let pid = libc::pid_t::try_from(pid).map_err(std::io::Error::other)?;
        // SAFETY: kill takes no pointers; a stale pid fails with ESRCH.
        if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let status = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("taskkill failed: {status}")))
        }
    }
}
