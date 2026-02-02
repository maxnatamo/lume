mod layout;
mod reloc;
mod write;

use derive_more::Debug;
use indexmap::IndexMap;
use lume_errors::{Result, SimpleDiagnostic};
use lume_span::{Internable, Interned};
use object::{NativeEndian as NE, macho};

use crate::common::*;
use crate::write::Writer;
use crate::{Context, EntryDisplay, SizedEntry, align_to};

/// Default entry point symbol name.
pub const DEFAULT_ENTRY: &str = "_main";

/// Name of the dynamic linker to use.
const DYLINKER_NAME: &str = "/usr/lib/dyld";

/// Default page zero size for the linker (only used on macOS).
pub const PAGE_ZERO_SIZE_64: u64 = 0x0000_0001_0000_0000;

/// Default page zero size for the linker (only used on macOS).
pub const PAGE_ZERO_SIZE_32: u64 = 0x0000_0000_0000_1000;

pub(crate) fn write<W: Writer>(ctx: Context<'_, Entry>, writer: &mut W) -> Result<()> {
    let entrypoint = ctx.config.entry.as_deref().unwrap_or(DEFAULT_ENTRY).to_string();
    let Some(entrypoint_symbol_id) = ctx.symbols.global_symbol(entrypoint.intern()) else {
        return Err(SimpleDiagnostic::new(format!("could not find symbol {entrypoint}")).into());
    };

    let string_table = merge_strings(&ctx);
    let symbol_table = define_symbols(&ctx);
    let libraries = ctx.required_library_ids();

    let mut layout = layout::Layout::new(ctx, string_table, symbol_table, libraries, entrypoint_symbol_id);
    layout.declare_layout();

    #[allow(clippy::disallowed_macros, reason = "used for non-logging purposes in the CLI")]
    if layout.ctx.config.print_entries {
        println!("{}", layout.ctx);
    }

    write::emit_to(writer, layout)?;

    Ok(())
}

#[derive(derive_more::Display, Debug, Clone)]
#[derive_where::derive_where(Hash, PartialEq, Eq)]
pub(crate) enum Entry {
    /// Header for the file format.
    #[display("file header")]
    FileHeader,

    /// Header for the page zero segment (__PAGEZERO).
    #[display("segment __PAGEZERO")]
    PageZero,

    /// Header for a single segment with the given name.
    #[display("segment {}", _0.name)]
    SegmentHeader(SegmentContent),

    /// Header for the `__LINKEDIT` segment.
    #[display("segment __LINKEDIT")]
    LinkEdit,

    /// Load dynamic library of the given ID
    #[display("dylib {_1}")]
    DylibHeader(LibraryId, #[derive_where(skip)] Interned<String>),

    /// Load command for the symbol table
    #[display("symtab")]
    SymtabHeader,

    /// Load command for the dynamic symbol table
    #[display("dysymtab")]
    DysymtabHeader,

    /// Table of all symbols in the file
    #[display("symbol table")]
    SymbolTable,

    /// Table of all interned strings in the file
    #[display("string table")]
    StringTable,

    /// Load command for the entrypoint address
    #[display("entrypoint")]
    Entrypoint,

    /// Load command for loading the dynamic linker
    #[display("dylinker")]
    LoadDylinker,

    /// UUID load command
    #[display("uuid")]
    Uuid,

    /// Build version load command
    #[display("build version")]
    BuildVersion,

    /// Source version load command
    #[display("source version")]
    SourceVersion,

    /// Data for a single section with the given ID.
    #[display("section")]
    SectionData(OutputSectionId),

    /// Padding for segment address alignment
    #[display("padding 0x{_0:0X}")]
    Padding(u64),
}

impl Entry {
    /// Determines if the entry is a load command.
    #[inline]
    pub fn is_load_command(&self) -> bool {
        matches!(
            self,
            Entry::PageZero
                | Entry::SegmentHeader(_)
                | Entry::LinkEdit
                | Entry::SymtabHeader
                | Entry::DysymtabHeader
                | Entry::DylibHeader(_, _)
                | Entry::Entrypoint
                | Entry::LoadDylinker
                | Entry::Uuid
                | Entry::BuildVersion
                | Entry::SourceVersion
        )
    }

    /// Determines if the entry is a section data entry.
    #[inline]
    pub fn is_section_data(&self) -> bool {
        matches!(self, Entry::SectionData(_))
    }
}

impl SizedEntry for Entry {
    fn physical_size(entry: &Self, ctx: &Context<'_, Self>) -> u64 {
        match entry {
            Entry::FileHeader => {
                if ctx.target.is_64bit() {
                    size_of::<macho::MachHeader64<NE>>() as u64
                } else {
                    size_of::<macho::MachHeader32<NE>>() as u64
                }
            }
            Entry::PageZero | Entry::LinkEdit => {
                if ctx.target.is_64bit() {
                    size_of::<macho::SegmentCommand64<NE>>() as u64
                } else {
                    size_of::<macho::SegmentCommand32<NE>>() as u64
                }
            }
            Entry::SegmentHeader(segment_content) => {
                let segment_size = if ctx.target.is_64bit() {
                    size_of::<macho::SegmentCommand64<NE>>() as u64
                } else {
                    size_of::<macho::SegmentCommand32<NE>>() as u64
                };

                let section_size = if ctx.target.is_64bit() {
                    size_of::<macho::Section64<NE>>() as u64
                } else {
                    size_of::<macho::Section32<NE>>() as u64
                };

                segment_size + section_size * segment_content.sections.len() as u64
            }
            Entry::SectionData(section_id) => ctx.db.size_of_section(*section_id),
            Entry::SymbolTable => {
                let nsyms = ctx.symbols.count() as u64;
                nsyms * size_of::<macho::Nlist64<NE>>() as u64
            }
            Entry::StringTable => {
                // First entry is a single space, used as a null string
                let mut strsize = 2_u64;

                for symbol_name in ctx.symbols.iter_names() {
                    strsize += symbol_name.len() as u64 + 1;
                }

                strsize
            }
            Entry::DylibHeader(_library_id, library_name) => {
                let mut dylib_size = 0;
                dylib_size += size_of::<macho::DylibCommand<NE>>() as u64;
                dylib_size += library_name.len() as u64 + 1;
                dylib_size = align_to(dylib_size, align_of::<u64>() as u64);

                dylib_size
            }
            Entry::SymtabHeader => size_of::<macho::SymtabCommand<NE>>() as u64,
            Entry::DysymtabHeader => size_of::<macho::DysymtabCommand<NE>>() as u64,
            Entry::Entrypoint => size_of::<macho::EntryPointCommand<NE>>() as u64,
            Entry::LoadDylinker => {
                let mut dylinker_size = size_of::<macho::DylinkerCommand<NE>>() as u64;
                dylinker_size += DYLINKER_NAME.len() as u64 + 1;
                dylinker_size = align_to(dylinker_size, align_of::<u64>() as u64);

                dylinker_size
            }
            Entry::Uuid => size_of::<macho::UuidCommand<NE>>() as u64,
            Entry::BuildVersion => size_of::<macho::BuildVersionCommand<NE>>() as u64,
            Entry::SourceVersion => size_of::<macho::SourceVersionCommand<NE>>() as u64,
            Entry::Padding(size) => *size,
        }
    }

    fn alignment(entry: &Self, ctx: &Context<'_, Self>) -> u64 {
        match entry {
            Entry::FileHeader
            | Entry::PageZero
            | Entry::SegmentHeader(_)
            | Entry::DylibHeader(_, _)
            | Entry::SymtabHeader
            | Entry::DysymtabHeader
            | Entry::Entrypoint
            | Entry::LoadDylinker
            | Entry::Uuid
            | Entry::BuildVersion
            | Entry::SourceVersion
            | Entry::Padding(_) => 1,
            Entry::LinkEdit | Entry::StringTable | Entry::SymbolTable => 4,
            Entry::SectionData(section_id) => ctx.db.output_section(*section_id).alignment as u64,
        }
    }
}

impl EntryDisplay for Entry {
    fn fmt(&self, ctx: &Context<'_, Entry>, w: &mut dyn std::fmt::Write) -> std::fmt::Result {
        match self {
            Self::FileHeader => write!(w, "FileHeader"),
            Self::PageZero => write!(w, "PageZero"),
            Self::SegmentHeader(segment) => write!(w, "SegmentHeader, {}", segment.name),
            Self::LinkEdit => write!(w, "Linkedit"),
            Self::SectionData(section_id) => {
                write!(w, "SectionData, {}", ctx.db.output_section(*section_id).name)
            }
            Self::SymbolTable => write!(w, "SymbolTable"),
            Self::StringTable => write!(w, "StringTable"),
            Self::DylibHeader(_library_id, library_name) => {
                write!(w, "DylibHeader, {library_name}")
            }
            Self::SymtabHeader => write!(w, "SymtabHeader"),
            Self::DysymtabHeader => write!(w, "DysymtabHeader"),
            Self::Entrypoint => write!(w, "Entrypoint"),
            Self::LoadDylinker => write!(w, "LoadDylinker"),
            Self::Uuid => write!(w, "Uuid"),
            Self::BuildVersion => write!(w, "BuildVersion"),
            Self::SourceVersion => write!(w, "SourceVersion"),
            Self::Padding(size) => write!(w, "Padding(0x{size:04X})"),
        }
    }
}

#[derive(Debug, Clone)]
#[derive_where::derive_where(Hash, PartialEq, Eq)]
pub(crate) struct SegmentContent {
    pub name: Interned<String>,

    #[derive_where(skip)]
    pub sections: Vec<OutputSectionId>,

    /// Sum of all contained section's data size without any additional padding.
    #[derive_where(skip)]
    pub data_size: u64,

    /// Sum of all contained section's data size, aligned to the required
    /// size alignment.
    #[derive_where(skip)]
    pub total_size: u64,
}

impl SegmentContent {
    pub fn new(name: Interned<String>) -> Self {
        SegmentContent {
            name,
            sections: Vec::new(),
            data_size: 0,
            total_size: 0,
        }
    }

    pub fn is_text(&self) -> bool {
        self.name.as_str() == macho::SEG_TEXT
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StringTable {
    /// Map of all strings in the table, mapped to their offset in the table.
    pub strings: IndexMap<Interned<String>, u64>,

    /// Total size of the string table, in bytes.
    pub total_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SymbolTable {
    pub symbols: Vec<Symbol>,

    /// Total size of the symbol table, in bytes.
    pub total_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Symbol {
    pub id: SymbolId,
    pub name: Interned<String>,
    pub ntype: u8,
    pub ndesc: u16,
    pub nsect: u8,
}

fn merge_strings(ctx: &Context<'_, Entry>) -> StringTable {
    let string_capacity = 1 + ctx.symbols.count();

    let mut strings = IndexMap::with_capacity(string_capacity);
    let mut total_size = 0;

    // First entry is a single space, used as a null string
    strings.insert(String::from(" ").intern(), 0);
    total_size += 2;

    for symbol_name in ctx.symbols.iter_names() {
        if strings.insert(symbol_name, total_size).is_none() {
            total_size += symbol_name.len() as u64 + 1;
        }
    }

    StringTable { strings, total_size }
}

/// Sorts the given iterator of symbols, depending on their linkage.
///
/// The symbol table in Mach-O expects the symbols to appear in a certain order:
/// - local debug symbols,
/// - private symbols,
/// - external symbols,
/// - undefined symbols
fn sort_symbols<I>(ctx: &Context<'_, Entry>, symbols: I) -> Vec<SymbolId>
where
    I: Iterator<Item = SymbolId>,
{
    let mut sorted_symbols: Vec<_> = symbols.collect();

    sorted_symbols.sort_by_key(|&sym_id| {
        let linkage = ctx.db.symbol(sym_id).unwrap().linkage;

        match linkage {
            Linkage::Local => 0,
            Linkage::Global => 1,
            Linkage::External => 2,
        }
    });

    sorted_symbols
}

fn define_symbols(ctx: &Context<'_, Entry>) -> SymbolTable {
    let sorted_symbols = sort_symbols(ctx, ctx.symbols.iter_ids());

    let nlist_size = if ctx.target.is_64bit() {
        size_of::<macho::Nlist64<NE>>() as u64
    } else {
        size_of::<macho::Nlist32<NE>>() as u64
    };

    let mut symbols = Vec::with_capacity(ctx.symbols.count());
    let mut total_size = 0;

    for symbol_id in sorted_symbols {
        let symbol = ctx.db.symbol(symbol_id).unwrap();

        let ntype = match symbol.linkage {
            crate::Linkage::External => macho::N_UNDF | macho::N_EXT,
            crate::Linkage::Global => macho::N_SECT | macho::N_EXT,
            crate::Linkage::Local => macho::N_SECT,
        };

        let nsect = symbol
            .section
            .and_then(|id| {
                for (idx, merged_section) in ctx.db.output_sections().enumerate() {
                    if merged_section.merged_from.contains(&id) {
                        return Some(u8::try_from(idx).unwrap() + 1);
                    }
                }

                None
            })
            .unwrap_or(0);

        let ndesc = match symbol.linkage {
            crate::Linkage::External if symbol.weak => macho::REFERENCE_FLAG_UNDEFINED_LAZY,
            crate::Linkage::External => macho::REFERENCE_FLAG_UNDEFINED_NON_LAZY,
            crate::Linkage::Global | crate::Linkage::Local => macho::REFERENCE_FLAG_DEFINED,
        };

        symbols.push(Symbol {
            id: symbol.id,
            name: symbol.name.to_string().intern(),
            ntype,
            ndesc,
            nsect,
        });

        total_size += nlist_size;
    }

    SymbolTable { symbols, total_size }
}
