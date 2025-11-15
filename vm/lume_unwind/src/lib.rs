use libc::c_void;

#[repr(C)]
#[allow(non_camel_case_types)]
pub(crate) struct _Unwind_Context {
    _private: [u8; 0],
}

#[repr(C)]
#[allow(non_camel_case_types)]
pub(crate) struct _Unwind_Exception {
    exception_class: u64,
    exception_cleanup: Option<extern "C" fn(_Unwind_Reason_Code, *mut _Unwind_Exception)>,
    private: [u64; 2],
}

#[repr(C)]
#[derive(Debug)]
#[allow(non_camel_case_types)]
pub(crate) enum _Unwind_Reason_Code {
    _URC_NO_REASON = 0,
    _URC_END_OF_STACK = 5,
}

unsafe extern "C" {
    fn _Unwind_Backtrace(
        trace: extern "C" fn(*mut _Unwind_Context, *mut c_void) -> _Unwind_Reason_Code,
        trace_param: *mut c_void,
    ) -> _Unwind_Reason_Code;

    fn _Unwind_RaiseException(ex: *mut _Unwind_Exception) -> _Unwind_Reason_Code;

    fn _Unwind_ForcedUnwind(
        ex: *mut _Unwind_Exception,
        stop: extern "C" fn(
            i32,
            i32,
            u64,
            *mut _Unwind_Exception,
            *mut _Unwind_Context,
            *mut c_void,
        ) -> _Unwind_Reason_Code,
        param: *mut c_void,
    ) -> _Unwind_Reason_Code;

    fn _Unwind_GetIP(ctx: *mut _Unwind_Context) -> usize;
}

extern "C" fn trace_fn(ctx: *mut _Unwind_Context, param: *mut c_void) -> _Unwind_Reason_Code {
    unsafe {
        let frames = &mut *(param as *mut Vec<usize>);
        let ip = _Unwind_GetIP(ctx);
        frames.push(ip);
    }

    _Unwind_Reason_Code::_URC_NO_REASON
}

pub fn backtrace() -> Vec<*const u8> {
    let mut frames = Vec::new();
    unsafe {
        _Unwind_Backtrace(trace_fn, &mut frames as *mut _ as *mut c_void);
    }

    frames
}

extern "C" fn stop_fn(
    _version: i32,
    _actions: i32,
    _class: u64,
    _ex: *mut _Unwind_Exception,
    ctx: *mut _Unwind_Context,
    param: *mut c_void,
) -> _Unwind_Reason_Code {
    unsafe {
        let frames = &mut *(param as *mut Vec<*const u8>);
        let ip = _Unwind_GetIP(ctx) as *const u8;
        frames.push(ip);
    }

    // Tell the unwinder to continue.
    _Unwind_Reason_Code::_URC_NO_REASON
}

pub fn backtrace_pure() -> Vec<*const u8> {
    unsafe {
        let mut frames = Vec::new();

        // Dummy exception object (unused except required by ABI)
        let mut ex = _Unwind_Exception {
            exception_class: 0x4E4F4E4558545243, // "NONEXTRC"
            exception_cleanup: None,
            private: [0, 0],
        };

        // Force unwind, collecting frames
        _Unwind_ForcedUnwind(&mut ex, stop_fn, &mut frames as *mut _ as *mut c_void);

        frames
    }
}
