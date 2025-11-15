use cranelift_codegen::isa::unwind::UnwindInfo;
use cranelift_jit::JITModule;
use gimli::write::{Address, EhFrame, Sections, Writer};
use lume_errors::Result;

use crate::CraneliftBackend;
use crate::dwarf::writer::WriterRelocate;
use crate::dwarf::{RootDebugContext, jit};

impl<'ctx> RootDebugContext<'ctx> {
    pub(crate) fn populate_frame_table(&mut self, backend: &CraneliftBackend, module: &mut JITModule) -> Result<()> {
        let mut cie = self.isa.create_systemv_cie().unwrap();
        cie.fde_address_encoding = gimli::DW_EH_PE_absptr;

        let cie_id = self.frame_table.add_cie(cie);

        for (node_id, unwind_info) in std::mem::take(&mut self.unwind_info) {
            let func_decl = backend.declared_funcs.get(&node_id).unwrap();
            let func_addr = module.get_finalized_function(func_decl.id).cast_mut();

            // println!("[{func_addr:p}] {}", func_decl.name);

            match unwind_info {
                UnwindInfo::SystemV(unwind_info) => {
                    let fde = unwind_info.to_fde(Address::Constant(func_addr.addr() as u64));

                    self.frame_table.add_fde(cie_id, fde);
                }
                UnwindInfo::WindowsArm64(_) | UnwindInfo::WindowsX64(_) => {}
                _ => unimplemented!(),
            }
        }

        Ok(())
    }

    pub(crate) fn register_frames<W: Writer>(&mut self, module: &JITModule, sections: &mut Sections<W>) -> Result<()> {
        use std::mem::ManuallyDrop;

        let mut eh_frame = EhFrame::from(WriterRelocate::new(self.endianess()));
        self.frame_table.write_eh_frame(&mut eh_frame).unwrap();

        self.frame_table.write_eh_frame(&mut sections.eh_frame).unwrap();
        self.frame_table.write_debug_frame(&mut sections.debug_frame).unwrap();

        if eh_frame.0.writer.slice().is_empty() {
            return Ok(());
        }

        let mut eh_frame = eh_frame.0.relocate_for_jit(module);

        // GCC expects a terminating "empty" length, so write a 0 length at the end of
        // the table.
        eh_frame.extend(&[0, 0, 0, 0]);

        // FIXME support unregistering unwind tables once cranelift-jit supports
        // deallocating individual functions
        let eh_frame = ManuallyDrop::new(eh_frame);

        // =======================================================================
        // Everything after this line up to the end of the file is loosely based on
        // https://github.com/bytecodealliance/wasmtime/blob/4471a82b0c540ff48960eca6757ccce5b1b5c3e4/crates/jit/src/unwind/systemv.rs
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
            unsafe { jit::__register_frame(eh_frame.as_ptr()) };
        }

        Ok(())
    }
}
