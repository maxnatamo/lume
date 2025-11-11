use cranelift_codegen::isa::unwind::UnwindInfo;
use cranelift_jit::JITModule;
use gimli::RunTimeEndian;
use gimli::write::{Address, EhFrame, EndianVec};
use lume_errors::{MapDiagnostic, Result};

use crate::CraneliftBackend;
use crate::dwarf::{RootDebugContext, jit};

impl<'ctx> RootDebugContext<'ctx> {
    pub(crate) fn populate_frame_table(&mut self, backend: &CraneliftBackend, module: &JITModule) -> Result<()> {
        for (node_id, unwind_info) in std::mem::take(&mut self.unwind_info) {
            let func_decl = backend.declared_funcs.get(&node_id).unwrap();
            let func_addr = module.get_finalized_function(func_decl.id).cast_mut();

            match unwind_info {
                UnwindInfo::SystemV(unwind_info) => {
                    let cie = self.isa.create_systemv_cie().unwrap();
                    let cie_id = self.frame_table.add_cie(cie);

                    let fde = unwind_info.to_fde(Address::Constant(func_addr.addr() as u64));
                    self.frame_table.add_fde(cie_id, fde);
                }
                UnwindInfo::WindowsArm64(_) | UnwindInfo::WindowsX64(_) => {}
                _ => unimplemented!(),
            }
        }

        Ok(())
    }

    pub(crate) fn register_frames(&mut self) -> Result<()> {
        use std::mem::ManuallyDrop;

        let mut eh_frame = EhFrame::from(EndianVec::<RunTimeEndian>::new(self.endianess()));
        self.frame_table.write_eh_frame(&mut eh_frame).map_diagnostic()?;

        if eh_frame.slice().is_empty() {
            return Ok(());
        }

        let mut eh_frame = eh_frame.take();
        eh_frame.extend(&[0, 0, 0, 0]);

        let eh_frame = ManuallyDrop::new(eh_frame);

        #[cfg(target_os = "macos")]
        unsafe {
            // On macOS, `__register_frame` takes a pointer to a single FDE
            let start = eh_frame.as_ptr();
            let end = start.add(eh_frame.len());
            let mut current = start;

            // Walk all of the entries in the frame table and register them
            while current < end {
                let len = std::ptr::read::<u32>(current as *const u32) as usize;

                // Skip over the CIE
                if current != start {
                    jit::__register_frame(current);
                }

                // Move to the next table entry (+4 because the length itself is not inclusive)
                current = current.add(len + 4);
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            // On other platforms, `__register_frame` will walk the FDEs until an entry of
            // length 0
            unsafe { __register_frame(eh_frame.as_ptr()) };
        }

        Ok(())
    }
}
