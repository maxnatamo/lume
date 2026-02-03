mod layout;
pub(crate) mod merge;
pub(crate) mod rules;
mod write;

use derive_more::Debug;
use indexmap::IndexMap;
use lume_errors::{Result, SimpleDiagnostic};
use lume_span::{Internable, Interned};
use object::{NativeEndian as NE, elf};
pub(crate) use rules::apply_rules;

use crate::common::*;
use crate::write::Writer;
use crate::{Context, EntryDisplay, SizedEntry};

/// Default entry point symbol name.
pub const DEFAULT_ENTRY: &str = "_start";

pub(crate) fn write<W: Writer>(ctx: Context<'_, Entry>, writer: &mut W) -> Result<()> {
    let entrypoint = ctx.config.entry.as_deref().unwrap_or(DEFAULT_ENTRY).to_string();
    let Some(entrypoint_symbol_id) = ctx.symbols.global_symbol(entrypoint.intern()) else {
        return Err(SimpleDiagnostic::new(format!("could not find symbol {entrypoint}")).into());
    };

    let string_table = merge_strings(&ctx);
    let symbol_table = define_symbols(&ctx);

    let mut layout = layout::Layout::new(ctx, string_table, symbol_table, entrypoint_symbol_id);
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

    /// Table of all program header entries in the file
    #[display("program table")]
    ProgramTable {
        #[derive_where(skip)]
        count: u64,
    },

    /// Table of all section header entries in the file
    #[display("section table")]
    SectionTable {
        #[derive_where(skip)]
        count: u64,
    },

    /// Data for a single section with the given ID.
    #[display("section")]
    SectionData(OutputSectionId),

    /// Table of all symbols in the file
    #[display("symbol table")]
    SymbolTable,

    /// Table of all interned strings in the file
    #[display("string table")]
    StringTable(#[derive_where(skip)] StringTable),
}

impl SizedEntry for Entry {
    fn physical_size(entry: &Self, ctx: &Context<'_, Self>) -> u64 {
        match entry {
            Entry::FileHeader => {
                if ctx.target.arch.is_64bit() {
                    size_of::<elf::FileHeader64<NE>>() as u64
                } else {
                    size_of::<elf::FileHeader32<NE>>() as u64
                }
            }
            Entry::ProgramTable { count } => {
                if ctx.target.arch.is_64bit() {
                    size_of::<elf::ProgramHeader64<NE>>() as u64 * count
                } else {
                    size_of::<elf::ProgramHeader32<NE>>() as u64 * count
                }
            }
            Entry::SectionTable { count } => {
                if ctx.target.arch.is_64bit() {
                    size_of::<elf::SectionHeader64<NE>>() as u64 * count
                } else {
                    size_of::<elf::SectionHeader32<NE>>() as u64 * count
                }
            }
            Entry::SymbolTable => {
                let nsyms = ctx.symbols.count() as u64 + 1;
                let entry_size = if ctx.target.arch.is_64bit() {
                    size_of::<elf::Sym64<NE>>() as u64
                } else {
                    size_of::<elf::Sym32<NE>>() as u64
                };

                nsyms * entry_size
            }
            Entry::StringTable(string_table) => string_table.total_size,
            Entry::SectionData(section_id) => ctx.db.size_of_section(*section_id),
        }
    }

    fn alignment(entry: &Self, ctx: &Context<'_, Self>) -> u64 {
        match entry {
            Entry::FileHeader
            | Entry::ProgramTable { .. }
            | Entry::SectionTable { .. }
            | Entry::StringTable(_)
            | Entry::SymbolTable => 1,
            Entry::SectionData(section_id) => ctx.db.output_section(*section_id).alignment as u64,
        }
    }
}

impl EntryDisplay for Entry {
    fn fmt(&self, ctx: &Context<'_, Entry>, w: &mut dyn std::fmt::Write) -> std::fmt::Result {
        match self {
            Self::FileHeader => write!(w, "FileHeader"),
            Self::ProgramTable { .. } => write!(w, "ProgramTable"),
            Self::SectionTable { .. } => write!(w, "SectionTable"),
            Self::StringTable(_) => write!(w, "StringTable"),
            Self::SymbolTable => write!(w, "SymbolTable"),
            Self::SectionData(section_id) => {
                write!(w, "SectionData, {}", ctx.db.output_section(*section_id).name)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StringTable {
    /// Set of all strings within the table.
    pub strings: IndexMap<Interned<String>, u64>,

    /// Total size of the string table, in bytes.
    pub total_size: u64,
}

impl StringTable {
    pub fn add_string(&mut self, string: Interned<String>) {
        match self.strings.entry(string) {
            indexmap::map::Entry::Occupied(_entry) => {}
            indexmap::map::Entry::Vacant(entry) => {
                entry.insert(self.total_size);

                self.total_size += string.len() as u64 + 1;
            }
        }
    }

    pub fn offset_of(&self, string: Interned<String>) -> u64 {
        self.strings.get(&string).copied().unwrap_or(0)
    }
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
    pub st_size: u64,
    pub st_info: u8,
    pub st_other: u8,
    pub st_shndx: u16,
}

fn merge_strings(ctx: &Context<'_, Entry>) -> StringTable {
    let string_capacity = 1 + ctx.symbols.count() + ctx.db.output_sections.len();

    let mut table = StringTable {
        strings: IndexMap::with_capacity(string_capacity),
        total_size: 0,
    };

    // First entry is a null string
    table.add_string(String::new().intern());

    for symbol_name in ctx.symbols.iter_names() {
        table.add_string(symbol_name);
    }

    for output_section in ctx.db.output_sections() {
        table.add_string(output_section.name.section);
    }

    table
}

fn define_symbols(ctx: &Context<'_, Entry>) -> SymbolTable {
    let entry_size = if ctx.target.arch.is_64bit() {
        size_of::<elf::Sym64<NE>>() as u64
    } else {
        size_of::<elf::Sym32<NE>>() as u64
    };

    let mut symbols = Vec::with_capacity(ctx.symbols.count());
    let mut total_size = entry_size;

    for symbol_id in ctx.symbols.iter_ids() {
        let symbol = ctx.db.symbol(symbol_id).unwrap();

        let st_other = match symbol.visibility {
            SymbolVisibility::Default => elf::STV_DEFAULT,
            SymbolVisibility::Protected => elf::STV_PROTECTED,
            SymbolVisibility::Hidden => elf::STV_HIDDEN,
        };

        let st_shndx = symbol
            .section
            .and_then(|id| {
                for (idx, merged_section) in ctx.db.output_sections().enumerate() {
                    if merged_section.merged_from.contains(&id) {
                        return Some(u16::try_from(idx).unwrap() + 1);
                    }
                }

                None
            })
            .unwrap_or(if let SymbolAddress::Absolute(_) = symbol.address {
                elf::SHN_ABS
            } else {
                elf::SHN_UNDEF
            });

        let st_bind = match symbol.linkage {
            _ if symbol.weak => elf::STB_WEAK,
            Linkage::Global => elf::STB_GLOBAL,
            Linkage::Local => elf::STB_LOCAL,
            Linkage::External => 0,
        };

        let st_type = elf::STT_NOTYPE;
        let st_info = (st_bind << 4) + (st_type & 0xf);

        symbols.push(Symbol {
            id: symbol.id,
            name: symbol.name.to_string().intern(),
            st_info,
            st_other,
            st_size: symbol.size as u64,
            st_shndx,
        });

        total_size += entry_size;
    }

    SymbolTable { symbols, total_size }
}
