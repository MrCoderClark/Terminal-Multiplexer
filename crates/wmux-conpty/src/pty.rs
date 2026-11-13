use std::fs::File;
use std::io;
use std::mem;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::time::Duration;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, UpdateProcThreadAttribute, WaitForSingleObject,
    EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    STARTUPINFOEXW,
};

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

pub struct Pty {
    hpc: HPCON,
    child: OwnedHandle,
    pid: u32,
    // Backing storage for the attribute list must live as long as the HPCON;
    // dropping it would free the memory that ConPTY still reads from on close.
    attr_list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _attr_list_buf: Vec<u8>,
}

pub struct PtyReader(File);
pub struct PtyWriter(File);

impl io::Read for PtyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl io::Write for PtyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

// PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE = ProcThreadAttributeValue(22, FALSE, TRUE, FALSE).
// The `windows` crate exposes it indirectly across versions, so keep the
// raw value here (22 | PROC_THREAD_ATTRIBUTE_INPUT=0x00020000).
const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x00020016;

impl Pty {
    pub fn spawn(command_line: &str, size: PtySize) -> Result<(Pty, PtyReader, PtyWriter)> {
        unsafe {
            let mut input_read = HANDLE::default();
            let mut input_write = HANDLE::default();
            let mut output_read = HANDLE::default();
            let mut output_write = HANDLE::default();

            CreatePipe(&mut input_read, &mut input_write, None, 0)
                .map_err(|e| Error::CreatePipe(io::Error::from_raw_os_error(e.code().0)))?;
            CreatePipe(&mut output_read, &mut output_write, None, 0)
                .map_err(|e| Error::CreatePipe(io::Error::from_raw_os_error(e.code().0)))?;

            let coord = COORD {
                X: size.cols as i16,
                Y: size.rows as i16,
            };
            let hpc = CreatePseudoConsole(coord, input_read, output_write, 0)
                .map_err(Error::CreatePseudoConsole)?;

            // After CreatePseudoConsole, the child side of each pipe is owned
            // by the pseudoconsole. Release our duplicates — otherwise the
            // output pipe never reaches EOF when the PTY is closed.
            let _ = CloseHandle(input_read);
            let _ = CloseHandle(output_write);

            // Size the attribute list. The first call is expected to fail with
            // ERROR_INSUFFICIENT_BUFFER but populates `size_needed`.
            let mut size_needed: usize = 0;
            let _ = InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST(ptr::null_mut()),
                1,
                0,
                &mut size_needed,
            );

            let mut attr_list_buf = vec![0u8; size_needed];
            let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_list_buf.as_mut_ptr() as *mut _);

            InitializeProcThreadAttributeList(attr_list, 1, 0, &mut size_needed)
                .map_err(Error::InitAttributeList)?;

            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
                Some(hpc.0 as *const _),
                mem::size_of::<HPCON>(),
                None,
                None,
            )
            .map_err(Error::UpdateAttribute)?;

            // CreateProcessW wants a *mutable* command line buffer (yes, really).
            let mut cmdline_utf16: Vec<u16> = command_line
                .encode_utf16()
                .chain(std::iter::once(0u16))
                .collect();

            let mut si = STARTUPINFOEXW::default();
            si.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
            si.lpAttributeList = attr_list;

            let mut pi = PROCESS_INFORMATION::default();

            CreateProcessW(
                PCWSTR::null(),
                PWSTR(cmdline_utf16.as_mut_ptr()),
                None,
                None,
                false,
                EXTENDED_STARTUPINFO_PRESENT,
                None,
                PCWSTR::null(),
                &si.StartupInfo,
                &mut pi,
            )
            .map_err(Error::CreateProcess)?;

            // We don't track the thread handle.
            let _ = CloseHandle(pi.hThread);

            let child = OwnedHandle::from_raw_handle(pi.hProcess.0 as _);
            let reader_file = File::from_raw_handle(output_read.0 as _);
            let writer_file = File::from_raw_handle(input_write.0 as _);

            Ok((
                Pty {
                    hpc,
                    child,
                    pid: pi.dwProcessId,
                    attr_list,
                    _attr_list_buf: attr_list_buf,
                },
                PtyReader(reader_file),
                PtyWriter(writer_file),
            ))
        }
    }

    pub fn resize(&self, size: PtySize) -> Result<()> {
        unsafe {
            ResizePseudoConsole(
                self.hpc,
                COORD {
                    X: size.cols as i16,
                    Y: size.rows as i16,
                },
            )
            .map_err(Error::ResizePseudoConsole)
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn wait(&self, timeout: Duration) -> Result<Option<u32>> {
        unsafe {
            let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
            let handle = HANDLE(self.child.as_raw_handle() as _);
            let r = WaitForSingleObject(handle, ms);
            if r == WAIT_OBJECT_0 {
                let mut code = 0u32;
                GetExitCodeProcess(handle, &mut code)
                    .map_err(|e| Error::Io(io::Error::from_raw_os_error(e.code().0)))?;
                Ok(Some(code))
            } else {
                Ok(None)
            }
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        unsafe {
            // Closing the pseudoconsole releases its internal pipe ends;
            // the client's PtyReader then sees EOF, which is how callers
            // know to stop reading.
            ClosePseudoConsole(self.hpc);
            DeleteProcThreadAttributeList(self.attr_list);
            // OwnedHandle on `child` closes the process handle.
        }
    }
}
