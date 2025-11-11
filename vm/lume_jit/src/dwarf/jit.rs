use std::ops::Deref;
use std::sync::Mutex;

#[repr(C)]
pub struct JITCodeEntry {
    pub next_entry: *mut JITCodeEntry,
    pub prev_entry: *mut JITCodeEntry,
    pub symfile_addr: *const u8,
    pub symfile_size: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct GlobalPointer<T>(*mut T);

impl<T> Deref for GlobalPointer<T> {
    type Target = *mut T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

unsafe impl<T> Send for GlobalPointer<T> {}
unsafe impl<T> Sync for GlobalPointer<T> {}

#[repr(C)]
pub struct JITDescriptor {
    pub version: u32,
    pub action_flag: i32,
    pub relevant_entry: *mut JITCodeEntry,
    pub first_entry: *mut JITCodeEntry,
}

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals, reason = "external object reference")]
pub static mut __jit_debug_descriptor: JITDescriptor = JITDescriptor {
    version: 1,
    action_flag: 0,
    relevant_entry: std::ptr::null_mut(),
    first_entry: std::ptr::null_mut(),
};

#[unsafe(no_mangle)]
pub extern "C" fn __jit_debug_register_code() {
    // Empty function — the debugger sets a breakpoint here.
}

static LIST: Mutex<Vec<GlobalPointer<JITCodeEntry>>> = Mutex::new(Vec::new());

pub(crate) fn register_jit_code(symfile_data: &[u8]) {
    unsafe {
        let symfile_addr = Box::leak(symfile_data.to_vec().into_boxed_slice());

        let entry = Box::into_raw(Box::new(JITCodeEntry {
            next_entry: std::ptr::null_mut(),
            prev_entry: std::ptr::null_mut(),
            symfile_addr: symfile_addr.as_ptr(),
            symfile_size: symfile_data.len() as u64,
        }));

        {
            let mut list = LIST.lock().unwrap();

            if let Some(head) = list.last() {
                (*head.0).next_entry = entry;
                (*entry).prev_entry = head.0;
            } else {
                __jit_debug_descriptor.first_entry = entry;
            }

            list.push(GlobalPointer(entry));
        }

        __jit_debug_descriptor.relevant_entry = entry;
        __jit_debug_descriptor.action_flag = 1; // JIT_REGISTER
        __jit_debug_register_code();
        __jit_debug_descriptor.action_flag = 0;
    }
}

unsafe extern "C" {
    pub fn __register_frame(fde: *const u8);
}
