use std::sync::OnceLock;

/// Gets the current memory page size for the operating system.
pub(crate) fn page_size() -> u64 {
    static PAGE_SIZE: OnceLock<u64> = OnceLock::new();

    *PAGE_SIZE.get_or_init(|| {
        #[cfg(unix)]
        {
            #[allow(clippy::cast_sign_loss, reason = "cannot be negative")]
            unsafe {
                libc::sysconf(libc::_SC_PAGESIZE) as u64
            }
        }

        #[cfg(windows)]
        {
            use winapi::um::sysinfoapi::{GetSystemInfo, LPSYSTEM_INFO, SYSTEM_INFO};

            unsafe {
                let mut info: SYSTEM_INFO = std::mem::zeroed();
                GetSystemInfo(&mut info as LPSYSTEM_INFO);

                info.dwPageSize
            }
        }
    })
}
