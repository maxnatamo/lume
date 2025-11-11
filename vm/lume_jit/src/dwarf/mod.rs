use std::collections::HashMap;
use std::sync::Arc;

use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::isa::unwind::UnwindInfo;
use cranelift_codegen::{Final, MachSrcLoc};
use gimli::Encoding;
use gimli::write::{Dwarf, FileId, FrameTable, UnitEntryId, UnitId};
use indexmap::IndexMap;
use lume_mir::ModuleMap;
use lume_span::{NodeId, SourceFileId};

pub mod debug_ctx;
pub mod jit;
pub mod unwind;

/// Context for creating DWARF debug info, which is defined
/// on the compilation unit itself, i.e. related to the package as
/// a whole.
pub(crate) struct RootDebugContext<'ctx> {
    ctx: &'ctx ModuleMap,
    isa: Arc<dyn TargetIsa>,
    dwarf: Dwarf,
    encoding: Encoding,

    file_units: IndexMap<SourceFileId, UnitId>,
    func_entries: IndexMap<NodeId, UnitEntryId>,
    func_mach_src: IndexMap<NodeId, Vec<MachSrcLoc<Final>>>,
    source_locations: IndexMap<SourceFileId, FileId>,

    frame_table: FrameTable,
    unwind_info: HashMap<NodeId, UnwindInfo>,
}

impl<'ctx> RootDebugContext<'ctx> {
    pub(crate) fn new(ctx: &'ctx ModuleMap, isa: Arc<dyn TargetIsa>) -> Self {
        let encoding = Encoding {
            format: gimli::Format::Dwarf32,
            version: 5,
            address_size: isa.frontend_config().pointer_bytes(),
        };

        let dwarf = Dwarf::new();

        let mut debug_ctx = Self {
            ctx,
            isa,
            dwarf,
            encoding,
            file_units: IndexMap::new(),
            func_entries: IndexMap::new(),
            func_mach_src: IndexMap::new(),
            source_locations: IndexMap::new(),
            frame_table: FrameTable::default(),
            unwind_info: HashMap::new(),
        };

        debug_ctx.create_compile_units();

        debug_ctx
    }

    /// Retrieves the source locations from the given function and places them
    /// into the DWARF unit.
    pub(crate) fn define_function(&mut self, func_id: NodeId, ctx: &cranelift::codegen::Context) {
        let mcr = ctx.compiled_code().unwrap();
        let mach_loc = mcr.buffer.get_srclocs_sorted().to_vec();
        self.func_mach_src.insert(func_id, mach_loc);

        if let Some(unwind_info) = ctx
            .compiled_code()
            .unwrap()
            .create_unwind_info(self.isa.as_ref())
            .unwrap()
        {
            self.unwind_info.insert(func_id, unwind_info);
        }
    }
}
