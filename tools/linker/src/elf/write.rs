use indexmap::IndexMap;
use lume_errors::Result;
use object::{NativeEndian as NE, elf};

use crate::align_to;
use crate::common::*;
use crate::elf::layout::Layout;
use crate::elf::{Entry, StringTable};
use crate::write::Writer;

pub(crate) fn emit_to<W: Writer>(writer: &mut W, mut layout: Layout<'_>) -> Result<()> {
    layout.virtual_places = layout_virtual_places(&layout, |entry| match &entry {
        _ => 0,
    });

    // layout.apply_relocations();

    for (entry, metadata) in layout.ctx.clone_entries() {
        let alignment = layout.ctx.alignment_of_entry(&entry);
        writer.align_to(alignment)?;

        let current_length = writer.len();

        let entry_offset = layout.ctx.offset_of_entry(&entry);
        assert_eq!(entry_offset, current_length as u64);

        match &entry {
            Entry::FileHeader => write_file_header(&layout, writer)?,
            Entry::ProgramTable { .. } => write_program_table(&layout, writer)?,
            Entry::SectionData(section_id) => write_section_data(&layout, *section_id, writer)?,
            Entry::SectionTable { .. } => write_section_table(&layout, writer)?,
            Entry::StringTable(table) => write_string_table(table, writer)?,
            Entry::SymbolTable => write_symbol_table(&layout, writer)?,
        }

        let written_bytes = writer.len() - current_length;
        assert!(
            metadata.physical_size == written_bytes as u64,
            "expected entry to be {} bytes, but wrote {} bytes: {entry:?}",
            metadata.physical_size,
            written_bytes,
        );
    }

    Ok(())
}

/// Calculates the virtual placement of all entries within the builder.
///
/// For each entry, the given closure is invoked to return the virtual size of
/// the entry. The virtual address of all subsequent entries is set to the sum
/// of all previous entry sizes.
fn layout_virtual_places<F: Fn(&Entry) -> u64>(layout: &Layout<'_>, f: F) -> IndexMap<Entry, Placement> {
    let mut vmaddr = 0;
    let mut entries = IndexMap::with_capacity(layout.ctx.entries().len());

    for (entry, _metadata) in layout.ctx.clone_entries() {
        let vmsize = f(&entry);

        entries.insert(entry, Placement {
            offset: vmaddr,
            size: vmsize,
        });

        vmaddr += vmsize;
    }

    entries
}

fn write_file_header<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    writer.write(&elf::ELFMAG)?; // ELF magic number
    writer.write_u8(layout.elf_class())?;
    writer.write_u8(layout.data_encoding())?;
    writer.write_u8(elf::EV_CURRENT)?;
    writer.write_u8(0x00)?;
    writer.write_u8(0x00)?;
    writer.write(&[0x0; 7])?;

    writer.write_u16(elf::ET_REL)?;
    writer.write_u16(layout.machine())?;
    writer.write_u32(u32::from(elf::EV_CURRENT))?;

    let e_entry = layout.vmaddr_of_symbol(layout.entrypoint);
    let e_phoff = layout.ctx.offset_of_entry(&Entry::ProgramTable { count: 0 });
    let e_shoff = layout.ctx.offset_of_entry(&Entry::SectionTable { count: 0 });
    let e_ehsize = layout.ctx.size_of_entry(&Entry::FileHeader);

    let e_phentsize = if layout.ctx.target.arch.is_64bit() {
        size_of::<elf::ProgramHeader64<NE>>() as u64
    } else {
        size_of::<elf::ProgramHeader32<NE>>() as u64
    };

    let e_shentsize = if layout.ctx.target.arch.is_64bit() {
        size_of::<elf::SectionHeader64<NE>>() as u64
    } else {
        size_of::<elf::SectionHeader32<NE>>() as u64
    };

    if layout.target().arch.is_64bit() {
        writer.write_u64(e_entry)?; // e_entry
        writer.write_u64(e_phoff)?; // e_phoff
        writer.write_u64(e_shoff)?; // e_shoff
    } else {
        writer.write_u32(u32::try_from(e_entry).unwrap())?; // e_entry
        writer.write_u32(u32::try_from(e_phoff).unwrap())?; // e_phoff
        writer.write_u32(u32::try_from(e_shoff).unwrap())?; // e_shoff
    }

    writer.write_u32(0x0000_0000)?; // e_flags

    writer.write_u16(u16::try_from(e_ehsize).unwrap())?;

    let e_phnum = layout.segments.len();
    writer.write_u16(u16::try_from(e_phentsize).unwrap())?; // e_phentsize
    writer.write_u16(u16::try_from(e_phnum).unwrap())?; // e_phnum

    let e_shnum = layout.ctx.db.output_sections.len() + 1;
    writer.write_u16(u16::try_from(e_shentsize).unwrap())?; // e_shentsize
    writer.write_u16(u16::try_from(e_shnum).unwrap())?; // e_shnum

    writer.write_u16(0x0000)?; // e_shstrndx

    Ok(())
}

fn write_program_table<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    for segment in &layout.segments {
        let sections = segment
            .sections
            .iter()
            .map(|&id| layout.ctx.db.output_section(id))
            .collect::<Vec<_>>();

        if layout.target().arch.is_64bit() {
            writer.write_u32(segment.segment_type.bits())?; // p_type
            writer.write_u32(segment.segment_flags.bits())?; // p_flags
            writer.write_u64(0x0000_0000_0000_0000)?; // p_offset
            writer.write_u64(0x0000_0000_0000_0000)?; // p_vaddr
            writer.write_u64(0x0000_0000_0000_0000)?; // p_paddr
            writer.write_u64(0x0000_0000_0000_0000)?; // p_filesz
            writer.write_u64(0x0000_0000_0000_0000)?; // p_memsz
            writer.write_u64(segment.alignment as u64)?; // p_align
        } else {
            writer.write_u32(segment.segment_type.bits())?; // p_type
            writer.write_u32(0x0000_0000)?; // p_offset
            writer.write_u32(0x0000_0000)?; // p_vaddr
            writer.write_u32(0x0000_0000)?; // p_paddr
            writer.write_u32(0x0000_0000)?; // p_filesz
            writer.write_u32(0x0000_0000)?; // p_memsz
            writer.write_u32(segment.segment_flags.bits())?; // p_flags
            writer.write_u32(u32::try_from(segment.alignment).unwrap())?; // p_align
        }
    }

    Ok(())
}

fn write_section_data<W: Writer>(layout: &Layout<'_>, section_id: OutputSectionId, writer: &mut W) -> Result<()> {
    let section = layout.ctx.db.output_section(section_id);
    let mut total_section_size = 0;

    for &contained_section_id in &section.merged_from {
        let contained_section = layout.ctx.db.input_section(contained_section_id);
        writer.write(&contained_section.data)?;

        total_section_size += contained_section.data.len() as u64;
    }

    let aligned_size = align_to(total_section_size, section.alignment as u64);
    let padding_size = aligned_size - total_section_size;

    if padding_size > 0 {
        writer.write(&vec![0x00; usize::try_from(padding_size).unwrap()])?;
    }

    Ok(())
}

fn write_section_table<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    // First section entry must be the "null" section
    if layout.target().arch.is_64bit() {
        writer.write_u32(0)?; // sh_name
        writer.write_u32(elf::SHT_NULL)?; // sh_type
        writer.write_u64(0)?; // sh_flags
        writer.write_u64(0)?; // sh_addr
        writer.write_u64(0)?; // sh_offset
        writer.write_u64(0)?; // sh_size
        writer.write_u32(0)?; // sh_link
        writer.write_u32(0)?; // sh_info
        writer.write_u64(0)?; // sh_addralign
        writer.write_u64(0x0000_0000_0000_0000)?; // sh_entsize
    } else {
        writer.write_u32(0)?; // sh_name
        writer.write_u32(elf::SHT_NULL)?; // sh_type
        writer.write_u32(0)?; // sh_flags
        writer.write_u32(0)?; // sh_addr
        writer.write_u32(0)?; // sh_offset
        writer.write_u32(0)?; // sh_size
        writer.write_u32(0)?; // sh_link
        writer.write_u32(0)?; // sh_info
        writer.write_u32(0)?; // sh_addralign
        writer.write_u32(0x0000_0000)?; // sh_entsize
    }

    for sec in &layout.sections {
        let data_entry = Entry::SectionData(sec.id);

        let sh_name = layout.string_table.offset_of(sec.name);
        let sh_addr = layout.vmaddr_of_entry(&data_entry);
        let sh_offset = layout.ctx.offset_of_entry(&data_entry);

        if layout.target().arch.is_64bit() {
            writer.write_u32(u32::try_from(sh_name).unwrap())?; // sh_name
            writer.write_u32(sec.sh_type)?; // sh_type
            writer.write_u64(u64::from(sec.sh_flags))?; // sh_flags
            writer.write_u64(sh_addr)?; // sh_addr
            writer.write_u64(sh_offset)?; // sh_offset
            writer.write_u64(sec.sh_size)?; // sh_size
            writer.write_u32(sec.sh_link)?; // sh_link
            writer.write_u32(sec.sh_info)?; // sh_info
            writer.write_u64(sec.sh_align)?; // sh_addralign
            writer.write_u64(0x0000_0000_0000_0000)?; // sh_entsize
        } else {
            writer.write_u32(u32::try_from(sh_name).unwrap())?; // sh_name
            writer.write_u32(sec.sh_type)?; // sh_type
            writer.write_u32(sec.sh_flags)?; // sh_flags
            writer.write_u32(u32::try_from(sh_addr).unwrap())?; // sh_addr
            writer.write_u32(u32::try_from(sh_offset).unwrap())?; // sh_offset
            writer.write_u32(u32::try_from(sec.sh_size).unwrap())?; // sh_size
            writer.write_u32(sec.sh_link)?; // sh_link
            writer.write_u32(sec.sh_info)?; // sh_info
            writer.write_u32(u32::try_from(sec.sh_align).unwrap())?; // sh_addralign
            writer.write_u32(0x0000_0000)?; // sh_entsize
        }
    }

    Ok(())
}

fn write_string_table<W: Writer>(string_table: &StringTable, writer: &mut W) -> Result<()> {
    for symbol_name in string_table.strings.keys() {
        writer.write(symbol_name.as_bytes())?;
        writer.write(&[0])?;
    }

    Ok(())
}

fn write_symbol_table<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    if layout.target().arch.is_64bit() {
        writer.write_u32(0)?; // st_name
        writer.write_u8(0)?; // st_info
        writer.write_u8(0)?; // st_other
        writer.write_u16(elf::SHN_UNDEF)?; // st_shndx
        writer.write_u64(0)?; // st_value
        writer.write_u64(0)?; // st_size
    } else {
        writer.write_u32(0)?; // st_name
        writer.write_u32(0)?; // st_value
        writer.write_u32(0)?; // st_size
        writer.write_u8(0)?; // st_info
        writer.write_u8(0)?; // st_other
        writer.write_u16(elf::SHN_UNDEF)?; // st_shndx
    }

    for symbol in &layout.symbol_table.symbols {
        let st_name = layout.string_table.offset_of(symbol.name);
        let st_value = layout.vmaddr_of_symbol(symbol.id);

        if layout.target().arch.is_64bit() {
            writer.write_u32(u32::try_from(st_name).unwrap())?; // st_name
            writer.write_u8(symbol.st_info)?; // st_info
            writer.write_u8(symbol.st_other)?; // st_other
            writer.write_u16(symbol.st_shndx)?; // st_shndx
            writer.write_u64(st_value)?; // st_value
            writer.write_u64(symbol.st_size)?; // st_size
        } else {
            writer.write_u32(u32::try_from(st_name).unwrap())?; // st_name
            writer.write_u32(u32::try_from(st_value).unwrap())?; // st_value
            writer.write_u32(u32::try_from(symbol.st_size).unwrap())?; // st_size
            writer.write_u8(symbol.st_info)?; // st_info
            writer.write_u8(symbol.st_other)?; // st_other
            writer.write_u16(symbol.st_shndx)?; // st_shndx
        }
    }

    Ok(())
}
