use crate::error::{EngineResult, RemoteEngineError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub type OutputCallback = Arc<dyn Fn(&[u8]) + Send + Sync>;
pub type ExitCallback = Arc<dyn Fn(Option<i32>) + Send + Sync>;

pub struct SpawnedPty {
    #[cfg(unix)]
    inner: unix_impl::UnixPty,
    #[cfg(windows)]
    inner: win_impl::WinPty,
    #[cfg(not(any(unix, windows)))]
    _unsupported: (),
    alive: Arc<AtomicBool>,
}

impl SpawnedPty {
    pub fn spawn(
        shell: &str,
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
        on_output: OutputCallback,
        on_exit: ExitCallback,
    ) -> EngineResult<Self> {
        let alive = Arc::new(AtomicBool::new(true));
        #[cfg(unix)]
        {
            let inner = unix_impl::UnixPty::spawn(
                shell,
                cols,
                rows,
                cwd,
                Arc::clone(&alive),
                on_output,
                on_exit,
            )?;
            return Ok(Self { inner, alive });
        }
        #[cfg(windows)]
        {
            let inner = win_impl::WinPty::spawn(
                shell,
                cols,
                rows,
                cwd,
                Arc::clone(&alive),
                on_output,
                on_exit,
            )?;
            return Ok(Self { inner, alive });
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (shell, cols, rows, cwd, on_output, on_exit, alive);
            Err(RemoteEngineError::Unavailable(
                "PTY unsupported on this platform".into(),
            ))
        }
    }

    pub fn write_all(&self, data: &[u8]) -> EngineResult<()> {
        #[cfg(unix)]
        return self.inner.write_all(data);
        #[cfg(windows)]
        return self.inner.write_all(data);
        #[cfg(not(any(unix, windows)))]
        {
            let _ = data;
            Err(RemoteEngineError::Unavailable("PTY unsupported".into()))
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> EngineResult<()> {
        #[cfg(unix)]
        return self.inner.resize(cols, rows);
        #[cfg(windows)]
        return self.inner.resize(cols, rows);
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (cols, rows);
            Err(RemoteEngineError::Unavailable("PTY unsupported".into()))
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn kill(&self) -> EngineResult<()> {
        #[cfg(unix)]
        return self.inner.kill();
        #[cfg(windows)]
        return self.inner.kill();
        #[cfg(not(any(unix, windows)))]
        Ok(())
    }
}

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use parking_lot::Mutex;
    use std::ffi::CString;
    use std::os::unix::io::RawFd;
    use std::path::Path;
    use std::ptr;

    pub struct UnixPty {
        write_fd: Mutex<RawFd>,
        child_pid: libc::pid_t,
        alive: Arc<AtomicBool>,
    }

    fn resolve_executable(cmd: &str) -> EngineResult<CString> {
        let trimmed = cmd.trim();
        let target = if trimmed.is_empty() { "/bin/sh" } else { trimmed };
        let candidate_path = Path::new(target);
        if candidate_path.is_absolute() || target.contains('/') {
            return CString::new(target)
                .map_err(|_| RemoteEngineError::InvalidArgument("shell path contains NUL".into()));
        }
        if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                let full = dir.join(target);
                if full.is_file() {
                    if let Some(s) = full.to_str() {
                        return CString::new(s).map_err(|_| {
                            RemoteEngineError::InvalidArgument("shell path contains NUL".into())
                        });
                    }
                }
            }
        }
        CString::new(target)
            .map_err(|_| RemoteEngineError::InvalidArgument("shell path contains NUL".into()))
    }

    impl UnixPty {
        pub fn spawn(
            shell: &str,
            cols: u16,
            rows: u16,
            cwd: Option<&str>,
            alive: Arc<AtomicBool>,
            on_output: OutputCallback,
            on_exit: ExitCallback,
        ) -> EngineResult<Self> {
            let exe_cstr = resolve_executable(shell)?;
            let arg0_cstr = exe_cstr.clone();
            let cwd_cstr = match cwd.map(str::trim).filter(|s| !s.is_empty()) {
                Some(dir) => Some(CString::new(dir).map_err(|_| {
                    RemoteEngineError::InvalidArgument("cwd contains NUL".into())
                })?),
                None => None,
            };

            let mut env_cstrings: Vec<CString> = Vec::new();
            let mut has_term = false;
            for (k, v) in std::env::vars() {
                if k == "TMUX" || k == "TMUX_PANE" || k == crate::terminal::FABUSHI_IN_MUX_ENV {
                    continue;
                }
                if k == "TERM" {
                    has_term = true;
                }
                if let Ok(entry) = CString::new(format!("{k}={v}")) {
                    env_cstrings.push(entry);
                }
            }
            if !has_term {
                env_cstrings.push(CString::new("TERM=xterm-256color").expect("valid cstr"));
            }
            env_cstrings.push(
                CString::new(format!("{}=1", crate::terminal::FABUSHI_IN_MUX_ENV))
                    .expect("valid cstr"),
            );

            let mut argv_ptrs: Vec<*const libc::c_char> = vec![arg0_cstr.as_ptr(), ptr::null()];
            let mut envp_ptrs: Vec<*const libc::c_char> =
                env_cstrings.iter().map(|c| c.as_ptr()).collect();
            envp_ptrs.push(ptr::null());

            let mut master_fd: RawFd = -1;
            let mut slave_fd: RawFd = -1;
            let winsz = libc::winsize {
                ws_row: rows.max(4),
                ws_col: cols.max(10),
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let rc = unsafe {
                libc::openpty(
                    &mut master_fd,
                    &mut slave_fd,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &winsz as *const libc::winsize as *mut libc::winsize,
                )
            };
            if rc != 0 || master_fd < 0 || slave_fd < 0 {
                return Err(RemoteEngineError::Io(std::io::Error::last_os_error()));
            }

            let pid = unsafe { libc::fork() };
            if pid < 0 {
                let err = std::io::Error::last_os_error();
                unsafe {
                    libc::close(master_fd);
                    libc::close(slave_fd);
                }
                return Err(RemoteEngineError::Io(err));
            }

            if pid == 0 {
                // Child process: async-signal-safe setup only (no heap allocation!).
                unsafe {
                    libc::close(master_fd);
                    libc::setsid();
                    #[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly", target_os = "linux"))]
                    {
                        let _ = libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0);
                    }
                    libc::dup2(slave_fd, libc::STDIN_FILENO);
                    libc::dup2(slave_fd, libc::STDOUT_FILENO);
                    libc::dup2(slave_fd, libc::STDERR_FILENO);
                    if slave_fd > libc::STDERR_FILENO {
                        libc::close(slave_fd);
                    }
                    if let Some(ref dir) = cwd_cstr {
                        let _ = libc::chdir(dir.as_ptr());
                    }
                    libc::execve(
                        exe_cstr.as_ptr(),
                        argv_ptrs.as_mut_ptr() as *const *const libc::c_char,
                        envp_ptrs.as_ptr(),
                    );
                    libc::_exit(127);
                }
            }

            // Parent process
            unsafe {
                libc::close(slave_fd);
            }
            let write_fd = unsafe { libc::dup(master_fd) };
            if write_fd < 0 {
                let err = std::io::Error::last_os_error();
                unsafe {
                    libc::close(master_fd);
                    libc::kill(pid, libc::SIGKILL);
                    libc::waitpid(pid, ptr::null_mut(), 0);
                }
                return Err(RemoteEngineError::Io(err));
            }

            let alive_reader = Arc::clone(&alive);
            std::thread::Builder::new()
                .name(format!("fabushi-pty-read-{pid}"))
                .spawn(move || {
                    let mut buf = [0u8; 8192];
                    loop {
                        let n = unsafe {
                            libc::read(master_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
                        };
                        if n > 0 {
                            on_output(&buf[..n as usize]);
                        } else if n == 0 {
                            break;
                        } else {
                            let err = std::io::Error::last_os_error();
                            if err.raw_os_error() == Some(libc::EINTR) {
                                continue;
                            }
                            break;
                        }
                    }
                    unsafe {
                        libc::close(master_fd);
                    }
                    let mut status: libc::c_int = 0;
                    let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
                    let exit_code = if waited == pid {
                        if libc::WIFEXITED(status) {
                            Some(libc::WEXITSTATUS(status))
                        } else if libc::WIFSIGNALED(status) {
                            Some(128 + libc::WTERMSIG(status))
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    alive_reader.store(false, Ordering::SeqCst);
                    on_exit(exit_code);
                })
                .map_err(RemoteEngineError::Io)?;

            Ok(Self {
                write_fd: Mutex::new(write_fd),
                child_pid: pid,
                alive,
            })
        }

        pub fn write_all(&self, data: &[u8]) -> EngineResult<()> {
            let fd = *self.write_fd.lock();
            if fd < 0 {
                return Err(RemoteEngineError::Other("PTY write fd closed".into()));
            }
            let mut offset = 0usize;
            while offset < data.len() {
                let n = unsafe {
                    libc::write(
                        fd,
                        data[offset..].as_ptr() as *const libc::c_void,
                        data.len() - offset,
                    )
                };
                if n > 0 {
                    offset += n as usize;
                } else if n < 0 {
                    let err = std::io::Error::last_os_error();
                    if err.raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    return Err(RemoteEngineError::Io(err));
                } else {
                    break;
                }
            }
            Ok(())
        }

        pub fn resize(&self, cols: u16, rows: u16) -> EngineResult<()> {
            let fd = *self.write_fd.lock();
            if fd < 0 {
                return Ok(());
            }
            let winsz = libc::winsize {
                ws_row: rows.max(4),
                ws_col: cols.max(10),
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let rc = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &winsz) };
            if rc != 0 {
                return Err(RemoteEngineError::Io(std::io::Error::last_os_error()));
            }
            Ok(())
        }

        pub fn kill(&self) -> EngineResult<()> {
            if self.alive.load(Ordering::SeqCst) {
                unsafe {
                    libc::kill(self.child_pid, libc::SIGHUP);
                    libc::kill(self.child_pid, libc::SIGKILL);
                }
            }
            let mut fd_guard = self.write_fd.lock();
            if *fd_guard >= 0 {
                unsafe {
                    libc::close(*fd_guard);
                }
                *fd_guard = -1;
            }
            Ok(())
        }
    }

    impl Drop for UnixPty {
        fn drop(&mut self) {
            let _ = self.kill();
        }
    }
}

#[cfg(windows)]
mod win_impl {
    use super::*;
    use parking_lot::Mutex;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    type HANDLE = *mut c_void;
    type HPCON = *mut c_void;
    type BOOL = i32;
    type DWORD = u32;
    type HRESULT = i32;

    const INVALID_HANDLE_VALUE: HANDLE = (-1isize) as HANDLE;
    const EXTENDED_STARTUPINFO_PRESENT: DWORD = 0x0008_0000;
    const CREATE_UNICODE_ENVIRONMENT: DWORD = 0x0000_0400;
    const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;
    const HANDLE_FLAG_INHERIT: DWORD = 0x0000_0001;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct COORD {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    struct SECURITY_ATTRIBUTES {
        n_length: DWORD,
        lp_security_descriptor: *mut c_void,
        b_inherit_handle: BOOL,
    }

    #[repr(C)]
    struct STARTUPINFOW {
        cb: DWORD,
        lp_reserved: *mut u16,
        lp_desktop: *mut u16,
        lp_title: *mut u16,
        dw_x: DWORD,
        dw_y: DWORD,
        dw_x_size: DWORD,
        dw_y_size: DWORD,
        dw_x_count_chars: DWORD,
        dw_y_count_chars: DWORD,
        dw_fill_attribute: DWORD,
        dw_flags: DWORD,
        w_show_window: u16,
        cb_reserved2: u16,
        lp_reserved2: *mut u8,
        h_std_input: HANDLE,
        h_std_output: HANDLE,
        h_std_error: HANDLE,
    }

    #[repr(C)]
    struct STARTUPINFOEXW {
        startup_info: STARTUPINFOW,
        lp_attribute_list: *mut c_void,
    }

    #[repr(C)]
    struct PROCESS_INFORMATION {
        h_process: HANDLE,
        h_thread: HANDLE,
        dw_process_id: DWORD,
        dw_thread_id: DWORD,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreatePipe(
            h_read_pipe: *mut HANDLE,
            h_write_pipe: *mut HANDLE,
            lp_pipe_attributes: *const SECURITY_ATTRIBUTES,
            n_size: DWORD,
        ) -> BOOL;
        fn SetHandleInformation(h_object: HANDLE, dw_mask: DWORD, dw_flags: DWORD) -> BOOL;
        fn CreatePseudoConsole(
            size: COORD,
            h_input: HANDLE,
            h_output: HANDLE,
            dw_flags: DWORD,
            ph_pc: *mut HPCON,
        ) -> HRESULT;
        fn ResizePseudoConsole(h_pc: HPCON, size: COORD) -> HRESULT;
        fn ClosePseudoConsole(h_pc: HPCON);
        fn InitializeProcThreadAttributeList(
            lp_attribute_list: *mut c_void,
            dw_attribute_count: DWORD,
            dw_flags: DWORD,
            lp_size: *mut usize,
        ) -> BOOL;
        fn UpdateProcThreadAttribute(
            lp_attribute_list: *mut c_void,
            dw_flags: DWORD,
            attribute: usize,
            lp_value: *mut c_void,
            cb_size: usize,
            lp_previous_value: *mut c_void,
            lp_return_size: *mut usize,
        ) -> BOOL;
        fn DeleteProcThreadAttributeList(lp_attribute_list: *mut c_void);
        fn CreateProcessW(
            lp_application_name: *const u16,
            lp_command_line: *mut u16,
            lp_process_attributes: *const SECURITY_ATTRIBUTES,
            lp_thread_attributes: *const SECURITY_ATTRIBUTES,
            b_inherit_handles: BOOL,
            dw_creation_flags: DWORD,
            lp_environment: *const c_void,
            lp_current_directory: *const u16,
            lp_startup_info: *const STARTUPINFOW,
            lp_process_information: *mut PROCESS_INFORMATION,
        ) -> BOOL;
        fn ReadFile(
            h_file: HANDLE,
            lp_buffer: *mut u8,
            n_number_of_bytes_to_read: DWORD,
            lp_number_of_bytes_read: *mut DWORD,
            lp_overlapped: *mut c_void,
        ) -> BOOL;
        fn WriteFile(
            h_file: HANDLE,
            lp_buffer: *const u8,
            n_number_of_bytes_to_write: DWORD,
            lp_number_of_bytes_written: *mut DWORD,
            lp_overlapped: *mut c_void,
        ) -> BOOL;
        fn TerminateProcess(h_process: HANDLE, u_exit_code: u32) -> BOOL;
        fn GetExitCodeProcess(h_process: HANDLE, lp_exit_code: *mut DWORD) -> BOOL;
        fn WaitForSingleObject(h_handle: HANDLE, dw_milliseconds: DWORD) -> DWORD;
        fn CloseHandle(h_object: HANDLE) -> BOOL;
    }

    pub struct WinPty {
        hpcon: Mutex<usize>,
        in_write: Mutex<usize>,
        h_process: Mutex<usize>,
        alive: Arc<AtomicBool>,
    }

    unsafe impl Send for WinPty {}
    unsafe impl Sync for WinPty {}

    fn to_wide_null(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn build_utf16_env_block() -> Vec<u16> {
        let mut block: Vec<u16> = Vec::new();
        for (k, v) in std::env::vars() {
            if k.eq_ignore_ascii_case("TMUX")
                || k.eq_ignore_ascii_case("TMUX_PANE")
                || k.eq_ignore_ascii_case(crate::terminal::FABUSHI_IN_MUX_ENV)
            {
                continue;
            }
            block.extend(std::ffi::OsStr::new(&format!("{k}={v}")).encode_wide());
            block.push(0);
        }
        block.extend(
            std::ffi::OsStr::new(&format!("{}=1", crate::terminal::FABUSHI_IN_MUX_ENV))
                .encode_wide(),
        );
        block.push(0);
        block.push(0);
        block
    }

    impl WinPty {
        pub fn spawn(
            shell: &str,
            cols: u16,
            rows: u16,
            cwd: Option<&str>,
            alive: Arc<AtomicBool>,
            on_output: OutputCallback,
            on_exit: ExitCallback,
        ) -> EngineResult<Self> {
            let mut in_read: HANDLE = ptr::null_mut();
            let mut in_write: HANDLE = ptr::null_mut();
            let mut out_read: HANDLE = ptr::null_mut();
            let mut out_write: HANDLE = ptr::null_mut();
            let sa = SECURITY_ATTRIBUTES {
                n_length: std::mem::size_of::<SECURITY_ATTRIBUTES>() as DWORD,
                lp_security_descriptor: ptr::null_mut(),
                b_inherit_handle: 1,
            };
            unsafe {
                if CreatePipe(&mut in_read, &mut in_write, &sa, 0) == 0
                    || CreatePipe(&mut out_read, &mut out_write, &sa, 0) == 0
                {
                    return Err(RemoteEngineError::Io(std::io::Error::last_os_error()));
                }
                SetHandleInformation(in_write, HANDLE_FLAG_INHERIT, 0);
                SetHandleInformation(out_read, HANDLE_FLAG_INHERIT, 0);
            }

            let mut hpcon: HPCON = ptr::null_mut();
            let size = COORD {
                x: cols.max(10) as i16,
                y: rows.max(4) as i16,
            };
            let hr = unsafe { CreatePseudoConsole(size, in_read, out_write, 0, &mut hpcon) };
            unsafe {
                CloseHandle(in_read);
                CloseHandle(out_write);
            }
            if hr != 0 || hpcon.is_null() {
                unsafe {
                    CloseHandle(in_write);
                    CloseHandle(out_read);
                }
                return Err(RemoteEngineError::Other(format!(
                    "CreatePseudoConsole failed with HRESULT {hr:#x}"
                )));
            }

            let mut attr_size: usize = 0;
            unsafe {
                InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut attr_size);
            }
            let mut attr_buf = vec![0u8; attr_size.max(64)];
            let attr_ptr = attr_buf.as_mut_ptr() as *mut c_void;
            unsafe {
                if InitializeProcThreadAttributeList(attr_ptr, 1, 0, &mut attr_size) == 0 {
                    ClosePseudoConsole(hpcon);
                    CloseHandle(in_write);
                    CloseHandle(out_read);
                    return Err(RemoteEngineError::Io(std::io::Error::last_os_error()));
                }
                if UpdateProcThreadAttribute(
                    attr_ptr,
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                    hpcon,
                    std::mem::size_of::<HPCON>(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                ) == 0
                {
                    DeleteProcThreadAttributeList(attr_ptr);
                    ClosePseudoConsole(hpcon);
                    CloseHandle(in_write);
                    CloseHandle(out_read);
                    return Err(RemoteEngineError::Io(std::io::Error::last_os_error()));
                }
            }

            let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
            si.startup_info.cb = std::mem::size_of::<STARTUPINFOEXW>() as DWORD;
            si.lp_attribute_list = attr_ptr;

            let cmd_target = if shell.trim().is_empty() {
                "powershell.exe"
            } else {
                shell.trim()
            };
            let mut cmd_wide = to_wide_null(cmd_target);
            let env_block = build_utf16_env_block();
            let cwd_wide = cwd.map(to_wide_null);
            let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

            let ok = unsafe {
                CreateProcessW(
                    ptr::null(),
                    cmd_wide.as_mut_ptr(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                    env_block.as_ptr() as *const c_void,
                    cwd_wide
                        .as_ref()
                        .map(|w| w.as_ptr())
                        .unwrap_or(ptr::null()),
                    &si.startup_info,
                    &mut pi,
                )
            };
            unsafe {
                DeleteProcThreadAttributeList(attr_ptr);
            }
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                unsafe {
                    ClosePseudoConsole(hpcon);
                    CloseHandle(in_write);
                    CloseHandle(out_read);
                }
                return Err(RemoteEngineError::Io(err));
            }
            unsafe {
                CloseHandle(pi.h_thread);
            }

            let out_read_usize = out_read as usize;
            let h_proc_usize = pi.h_process as usize;
            let alive_reader = Arc::clone(&alive);
            std::thread::Builder::new()
                .name("fabushi-conpty-read".into())
                .spawn(move || {
                    let out_h = out_read_usize as HANDLE;
                    let proc_h = h_proc_usize as HANDLE;
                    let mut buf = [0u8; 8192];
                    loop {
                        let mut read_bytes: DWORD = 0;
                        let ok = unsafe {
                            ReadFile(
                                out_h,
                                buf.as_mut_ptr(),
                                buf.len() as DWORD,
                                &mut read_bytes,
                                ptr::null_mut(),
                            )
                        };
                        if ok == 0 || read_bytes == 0 {
                            break;
                        }
                        on_output(&buf[..read_bytes as usize]);
                    }
                    unsafe {
                        CloseHandle(out_h);
                        WaitForSingleObject(proc_h, 2_000);
                    }
                    let mut code: DWORD = 0;
                    let got = unsafe { GetExitCodeProcess(proc_h, &mut code) };
                    alive_reader.store(false, Ordering::SeqCst);
                    on_exit(if got != 0 { Some(code as i32) } else { None });
                })
                .map_err(RemoteEngineError::Io)?;

            Ok(Self {
                hpcon: Mutex::new(hpcon as usize),
                in_write: Mutex::new(in_write as usize),
                h_process: Mutex::new(pi.h_process as usize),
                alive,
            })
        }

        pub fn write_all(&self, data: &[u8]) -> EngineResult<()> {
            let h = *self.in_write.lock() as HANDLE;
            if h.is_null() || h == INVALID_HANDLE_VALUE {
                return Err(RemoteEngineError::Other("ConPTY input closed".into()));
            }
            let mut written: DWORD = 0;
            let ok = unsafe {
                WriteFile(
                    h,
                    data.as_ptr(),
                    data.len() as DWORD,
                    &mut written,
                    ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(RemoteEngineError::Io(std::io::Error::last_os_error()));
            }
            Ok(())
        }

        pub fn resize(&self, cols: u16, rows: u16) -> EngineResult<()> {
            let hp = *self.hpcon.lock() as HPCON;
            if hp.is_null() {
                return Ok(());
            }
            unsafe {
                ResizePseudoConsole(
                    hp,
                    COORD {
                        x: cols.max(10) as i16,
                        y: rows.max(4) as i16,
                    },
                );
            }
            Ok(())
        }

        pub fn kill(&self) -> EngineResult<()> {
            let mut hp = self.hpcon.lock();
            if *hp != 0 {
                unsafe {
                    ClosePseudoConsole(*hp as HPCON);
                }
                *hp = 0;
            }
            let mut iw = self.in_write.lock();
            if *iw != 0 {
                unsafe {
                    CloseHandle(*iw as HANDLE);
                }
                *iw = 0;
            }
            let mut proc_h = self.h_process.lock();
            if *proc_h != 0 {
                if self.alive.load(Ordering::SeqCst) {
                    unsafe {
                        TerminateProcess(*proc_h as HANDLE, 1);
                    }
                }
                unsafe {
                    CloseHandle(*proc_h as HANDLE);
                }
                *proc_h = 0;
            }
            Ok(())
        }
    }

    impl Drop for WinPty {
        fn drop(&mut self) {
            let _ = self.kill();
        }
    }
}
