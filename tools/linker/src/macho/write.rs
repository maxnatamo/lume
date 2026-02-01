use indexmap::IndexMap;
use lume_errors::Result;
use object::{NativeEndian as NE, macho};

use crate::common::*;
use crate::macho::layout::Layout;
use crate::macho::{DYLINKER_NAME, Entry, SegmentContent};
use crate::write::Writer;
use crate::{align_to, page_align};

pub(crate) fn emit_to<W: Writer>(writer: &mut W, mut layout: Layout<'_>) -> Result<()> {
    layout.virtual_places = layout_virtual_places(&layout, |entry| match &entry {
        Entry::PageZero => {
            if layout.target().is_64bit() {
                super::PAGE_ZERO_SIZE_64
            } else {
                super::PAGE_ZERO_SIZE_32
            }
        }

        // The `__LINKEDIT` segment currently contains the string table and the symbol table.
        Entry::LinkEdit => {
            let vbase = layout.ctx.offset_of_entry(&Entry::StringTable);

            let symtab_offset = layout.ctx.offset_of_entry(&Entry::SymbolTable);
            let symtab_size = layout.ctx.size_of_entry(&Entry::SymbolTable);
            let vend = symtab_offset + symtab_size;

            page_align(vend - vbase)
        }

        Entry::SegmentHeader(segment_content) => layout.aligned_segment_size(&segment_content.name),

        _ => 0,
    });

    layout.apply_relocations();

    for (entry, metadata) in layout.ctx.clone_entries() {
        let alignment = layout.ctx.alignment_of_entry(&entry);
        writer.align_to(alignment)?;

        let current_length = writer.len();

        let entry_offset = layout.ctx.offset_of_entry(&entry);
        assert_eq!(entry_offset, current_length as u64);

        match &entry {
            Entry::FileHeader => write_file_header(&layout, writer)?,
            Entry::PageZero => write_page_zero(&layout, writer)?,
            Entry::SegmentHeader(segment) => write_segment_header(&layout, &entry, segment, writer)?,
            Entry::LinkEdit => write_linkedit(&layout, writer)?,
            Entry::SymtabHeader => write_symtab_header(&layout, writer)?,
            Entry::DysymtabHeader => write_dysymtab_header(&layout, writer)?,
            Entry::DylibHeader(lib_id, _lib_name) => write_dylib_header(&layout, *lib_id, writer)?,
            Entry::SectionData(section_id) => write_section_data(&layout, *section_id, writer)?,
            Entry::StringTable => write_string_table(&layout, writer)?,
            Entry::SymbolTable => write_symbol_table(&layout, writer)?,
            Entry::Entrypoint => write_entrypoint(&layout, writer)?,
            Entry::LoadDylinker => write_dylinker(writer)?,
            Entry::Uuid => write_uuid(&layout, writer)?,
            Entry::BuildVersion => write_build_version(writer)?,
            Entry::SourceVersion => write_source_version(writer)?,
            Entry::Padding(size) => writer.write(&vec![0x00; usize::try_from(*size).unwrap()])?,
        }

        let written_bytes = writer.len() - current_length;
        assert!(
            metadata.physical_size == written_bytes as u64,
            "expected entry to be {} bytes, but wrote {} bytes: {entry:?}",
            metadata.physical_size,
            written_bytes
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
    writer.write_u32(layout.magic_number())?;
    writer.write_u32(layout.cpu_type())?;
    writer.write_u32(layout.cpu_subtype())?;

    writer.write_u32(macho::MH_EXECUTE)?;

    writer.write_u32(layout.lc_count())?;
    writer.write_u32(layout.lc_size())?;

    let flags = macho::MH_DYLDLINK | macho::MH_PIE | macho::MH_NOUNDEFS;
    writer.write_u32(flags)?;

    if layout.target().is_64bit() {
        writer.write_u32(0)?; // reserved (64-bit only)
    }

    Ok(())
}

fn write_page_zero<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    let lc_size = layout.ctx.size_of_entry(&Entry::PageZero);
    let vmsize = layout.vmsize_of_entry(&Entry::PageZero);

    writer.write_u32(macho::LC_SEGMENT_64)?;
    writer.write_u32(u32::try_from(lc_size).unwrap())?;

    let mut segment_name_bytes = macho::SEG_PAGEZERO.as_bytes().to_vec();
    segment_name_bytes.resize(16, 0);
    writer.write(&segment_name_bytes)?;

    writer.write_u64(0x0000_0000)?; // vmaddr
    writer.write_u64(vmsize)?; // vmsize

    writer.write_u64(0)?; // fileoff
    writer.write_u64(0)?; // filesize

    writer.write_u32(0)?; // maxprot
    writer.write_u32(0)?; // initprot

    writer.write_u32(0)?; // nsects
    writer.write_u32(0x00)?; // flags

    Ok(())
}

fn write_segment_header<W: Writer>(
    layout: &Layout<'_>,
    entry: &Entry,
    segment: &SegmentContent,
    writer: &mut W,
) -> Result<()> {
    let lc_size = layout.ctx.size_of_entry(entry);

    let seg_vmaddr = layout.vmaddr_of_entry(entry);
    let seg_vmsize = layout.vmsize_of_entry(entry);

    let fileoff = if segment.is_text() {
        0
    } else {
        segment
            .sections
            .first()
            .map_or(0, |&section| layout.ctx.offset_of_entry(&Entry::SectionData(section)))
    };

    writer.write_u32(macho::LC_SEGMENT_64)?;
    writer.write_u32(u32::try_from(lc_size).unwrap())?;

    let mut segment_name_bytes = segment.name.as_bytes().to_vec();
    segment_name_bytes.resize(16, 0);
    writer.write(&segment_name_bytes)?;

    writer.write_u64(seg_vmaddr)?;
    writer.write_u64(seg_vmsize)?;

    writer.write_u64(fileoff)?;
    writer.write_u64(seg_vmsize)?;

    let section_prot = match segment.name.as_str() {
        macho::SEG_TEXT => macho::VM_PROT_READ | macho::VM_PROT_EXECUTE,
        macho::SEG_DATA => macho::VM_PROT_READ | macho::VM_PROT_WRITE,
        _ => macho::VM_PROT_READ,
    };

    writer.write_u32(section_prot)?; // maxprot
    writer.write_u32(section_prot)?; // initprot

    writer.write_u32(u32::try_from(segment.sections.len()).unwrap())?; // nsects
    writer.write_u32(0x00)?; // flags

    for &section_id in &segment.sections {
        let data_entry = Entry::SectionData(section_id);
        let section = layout.ctx.db.output_section(section_id);

        let mut section_name_bytes = section.name.section.as_bytes().to_vec();
        section_name_bytes.resize(16, 0);
        writer.write(&section_name_bytes)?;

        writer.write(&segment_name_bytes)?;

        let sec_vmaddr = layout.vmaddr_of_section_data(segment, section_id);
        let sec_vmsize = layout.ctx.size_of_entry(&data_entry);
        let offset = layout.ctx.offset_of_entry(&data_entry);

        writer.write_u64(sec_vmaddr)?; // addr
        writer.write_u64(sec_vmsize)?; // size

        writer.write_u32(u32::try_from(offset).unwrap())?; // offset
        writer.write_u32(section.alignment.ilog2())?; // align

        writer.write_u32(0)?; // reloff
        writer.write_u32(0)?; // nreloc

        let flags = match section.kind {
            SectionKind::Unknown | SectionKind::Data => macho::S_REGULAR,
            SectionKind::Text => macho::S_ATTR_SOME_INSTRUCTIONS | macho::S_ATTR_PURE_INSTRUCTIONS,
            SectionKind::ZeroFilled => macho::S_ZEROFILL,
            SectionKind::CStrings => macho::S_CSTRING_LITERALS,
            SectionKind::LumeMetadata => macho::S_ATTR_NO_DEAD_STRIP,
            SectionKind::LumeAliases => macho::S_LITERAL_POINTERS,
        };

        writer.write_u32(flags)?;

        writer.write_u32(0)?; // reserved1
        writer.write_u32(0)?; // reserved2
        writer.write_u32(0)?; // reserved3
    }

    Ok(())
}

fn write_linkedit<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    let lc_size = layout.ctx.size_of_entry(&Entry::LinkEdit);

    let vmaddr = layout.vmaddr_of_entry(&Entry::LinkEdit);
    let vmsize = layout.vmsize_of_entry(&Entry::LinkEdit);
    let fileoff = layout.ctx.offset_of_entry(&Entry::StringTable);

    writer.write_u32(macho::LC_SEGMENT_64)?;
    writer.write_u32(u32::try_from(lc_size).unwrap())?;

    let mut segment_name_bytes = macho::SEG_LINKEDIT.as_bytes().to_vec();
    segment_name_bytes.resize(16, 0);
    writer.write(&segment_name_bytes)?;

    writer.write_u64(vmaddr)?; // vmaddr
    writer.write_u64(vmsize)?; // vmsize

    writer.write_u64(fileoff)?; // fileoff
    writer.write_u64(vmsize)?; // filesize

    writer.write_u32(macho::VM_PROT_READ)?; // maxprot
    writer.write_u32(macho::VM_PROT_READ)?; // initprot

    writer.write_u32(0)?; // nsects
    writer.write_u32(0x00)?; // flags

    Ok(())
}

fn write_symtab_header<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    let lc_size = size_of::<macho::SymtabCommand<NE>>();

    let symoff = layout.ctx.offset_of_entry(&Entry::SymbolTable);
    let nsyms = layout.ctx.symbols.count();

    let stroff = layout.ctx.offset_of_entry(&Entry::StringTable);
    let strsize = layout.ctx.size_of_entry(&Entry::StringTable);

    writer.write_u32(macho::LC_SYMTAB)?;
    writer.write_u32(u32::try_from(lc_size).unwrap())?;

    writer.write_u32(u32::try_from(symoff).unwrap())?;
    writer.write_u32(u32::try_from(nsyms).unwrap())?;

    writer.write_u32(u32::try_from(stroff).unwrap())?;
    writer.write_u32(u32::try_from(strsize).unwrap())?;

    Ok(())
}

fn write_dysymtab_header<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    let mut local_sym_len = 0_u32;
    let mut ext_sym_len = 0_u32;

    for symbol in &layout.symbol_table.symbols {
        let linkage = layout.ctx.db.symbol(symbol.id).unwrap().linkage;

        match linkage {
            Linkage::Local | Linkage::Global => {
                local_sym_len += 1;
            }
            Linkage::External => {
                ext_sym_len += 1;
            }
        }
    }

    let lc_size = size_of::<macho::DysymtabCommand<NE>>();

    writer.write_u32(macho::LC_DYSYMTAB)?;
    writer.write_u32(u32::try_from(lc_size).unwrap())?;

    writer.write_u32(0)?; // ilocalsym
    writer.write_u32(local_sym_len)?; // nlocalsym

    writer.write_u32(local_sym_len)?; // iextdefsym
    writer.write_u32(ext_sym_len)?; // nextdefsym

    writer.write_u32(local_sym_len)?; // iundefsym
    writer.write_u32(0)?; // nundefsym

    writer.write_u32(0)?; // tocoff
    writer.write_u32(0)?; // ntoc

    writer.write_u32(0)?; // modtaboff
    writer.write_u32(0)?; // nmodtab

    writer.write_u32(0)?; // extrefsymoff
    writer.write_u32(0)?; // nextrefsyms

    writer.write_u32(0)?; // indirectsymoff
    writer.write_u32(0)?; // nindirectsyms

    writer.write_u32(0)?; // extreloff
    writer.write_u32(0)?; // nextrel

    writer.write_u32(0)?; // locreloff
    writer.write_u32(0)?; // nlocrel

    Ok(())
}

fn write_dylib_header<W: Writer>(layout: &Layout<'_>, library_id: LibraryId, writer: &mut W) -> Result<()> {
    let library = layout.ctx.db.framework(library_id);
    let library_path = library.path.display().to_string();

    let name_size = library_path.len() + 1;

    let lc_size = size_of::<macho::DylibCommand<NE>>() + name_size;
    let lc_size = align_to(lc_size as u64, align_of::<u64>() as u64);

    writer.write_u32(macho::LC_LOAD_DYLIB)?;
    writer.write_u32(u32::try_from(lc_size).unwrap())?;

    // The library name is placed right after the load command
    writer.write_u32(u32::try_from(size_of::<macho::DylibCommand<NE>>()).unwrap())?; // name
    writer.write_u32(0x0000_0002)?; // timestamp
    writer.write_u32(0x054C_0000)?; // current_version
    writer.write_u32(0x0001_0000)?; // compatibility_version

    writer.write(library_path.as_bytes())?;
    writer.write_u8(0)?;

    // `otool` claims the `dylib` commands must be padded to a multiple of 4 bytes,
    // while `nm` requires padding to a multiple of 8 bytes.
    writer.align_to(align_of::<u64>() as u64)?;

    Ok(())
}

fn write_section_data<W: Writer>(layout: &Layout<'_>, id: OutputSectionId, writer: &mut W) -> Result<()> {
    let section = layout.ctx.db.output_section(id);

    for &contained_section_id in &section.merged_from {
        let contained_section = layout.ctx.db.input_section(contained_section_id);
        writer.write(&contained_section.data)?;
    }

    Ok(())
}

fn write_string_table<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    for (symbol_name, _symbol_offset) in &layout.string_table.strings {
        writer.write(symbol_name.as_bytes())?;
        writer.write(&[0])?;
    }

    Ok(())
}

fn write_symbol_table<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    for symbol in &layout.symbol_table.symbols {
        let nstrx = *layout.string_table.strings.get(&symbol.name).unwrap();

        writer.write_u32(u32::try_from(nstrx).unwrap())?;
        writer.write_u8(symbol.ntype)?;
        writer.write_u8(symbol.nsect)?;
        writer.write_u16(symbol.ndesc)?;
        writer.write_u64(layout.vmaddr_of_symbol(symbol.id))?;
    }

    Ok(())
}

fn write_entrypoint<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    let lc_size = size_of::<macho::EntryPointCommand<NE>>();

    writer.write_u32(macho::LC_MAIN)?;
    writer.write_u32(u32::try_from(lc_size).unwrap())?;

    let entryoff = layout.offset_of_symbol(layout.entrypoint).unwrap();
    let stacksize = layout.ctx.config.stack_size.unwrap_or(0);

    writer.write_u64(entryoff)?; // entryoff
    writer.write_u64(stacksize)?; // stacksize

    Ok(())
}

fn write_dylinker<W: Writer>(writer: &mut W) -> Result<()> {
    let cmd_size = size_of::<macho::DylinkerCommand<NE>>() as u64;
    let lc_size = align_to(cmd_size + DYLINKER_NAME.len() as u64 + 1, align_of::<u64>() as u64);

    writer.write_u32(macho::LC_LOAD_DYLINKER)?;
    writer.write_u32(u32::try_from(lc_size).unwrap())?;

    // The linker name is placed right after the load command
    writer.write_u32(u32::try_from(cmd_size).unwrap())?; // name

    writer.write(DYLINKER_NAME.as_bytes())?;
    writer.write_u8(0)?;

    writer.align_to(align_of::<u64>() as u64)?;

    Ok(())
}

fn write_uuid<W: Writer>(layout: &Layout<'_>, writer: &mut W) -> Result<()> {
    let lc_size = size_of::<macho::UuidCommand<NE>>() as u64;

    writer.write_u32(macho::LC_UUID)?;
    writer.write_u32(u32::try_from(lc_size).unwrap())?;

    let mut uuid_hi = 0_u64;
    let mut uuid_lo = 0_u64;

    for file_id in layout.ctx.db.files.keys() {
        uuid_hi = lume_span::hash_id(&(uuid_hi, file_id)) as u64;
    }

    for object in layout.ctx.db.objects.values() {
        uuid_lo = lume_span::hash_id(&(uuid_lo, object.id)) as u64;
    }

    assert_eq!(uuid_hi.to_ne_bytes().len(), 8);
    assert_eq!(uuid_lo.to_ne_bytes().len(), 8);

    writer.write(&uuid_hi.to_ne_bytes())?;
    writer.write(&uuid_lo.to_ne_bytes())?;

    Ok(())
}

fn write_build_version<W: Writer>(writer: &mut W) -> Result<()> {
    let lc_size = size_of::<macho::BuildVersionCommand<NE>>() as u64;

    writer.write_u32(macho::LC_BUILD_VERSION)?;
    writer.write_u32(u32::try_from(lc_size).unwrap())?;

    writer.write_u32(macho::PLATFORM_MACOS)?; // platform
    writer.write_u32(0x001A_0000)?; // minos
    writer.write_u32(0x001A_0200)?; // sdk
    writer.write_u32(0)?; // ntools

    Ok(())
}

fn write_source_version<W: Writer>(writer: &mut W) -> Result<()> {
    let lc_size = size_of::<macho::SourceVersionCommand<NE>>() as u64;

    writer.write_u32(macho::LC_SOURCE_VERSION)?;
    writer.write_u32(u32::try_from(lc_size).unwrap())?;
    writer.write_u64(0)?; // version

    Ok(())
}
