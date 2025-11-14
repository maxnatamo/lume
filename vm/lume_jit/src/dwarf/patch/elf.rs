use lume_errors::{MapDiagnostic, Result};
use object::NativeEndian;
use object::elf::FileHeader64;
use object::read::elf::ElfFile;

/// `object` restricts which attributes can be defined as a custom value, so we
/// manually patch the ELF binary.
///
/// This operation MUST be done in-memory and without copying the file content.
pub(crate) fn patch_binary_file(bytes: &mut [u8], code_start: *const u8, code_size: usize) -> Result<()> {
    const SECTION_HEADER_SIZE: usize = 64;

    const TEXT_SECTION_TYPE: u32 = object::elf::SHT_PROGBITS;
    const TEXT_SECTION_FLAGS: u64 = (object::elf::SHF_ALLOC | object::elf::SHF_EXECINSTR) as u64;

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        let slice = &bytes[offset..offset + 4];
        let arr: &[u8; 4] = slice.try_into().unwrap();

        u32::from_ne_bytes(*arr)
    }

    fn read_u64(bytes: &[u8], offset: usize) -> u64 {
        let slice = &bytes[offset..offset + 8];
        let arr: &[u8; 8] = slice.try_into().unwrap();

        u64::from_ne_bytes(*arr)
    }

    fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    let file = ElfFile::<FileHeader64<NativeEndian>>::parse(bytes as &[u8]).map_diagnostic()?;
    let section_num = file.elf_header().e_shnum.get(NativeEndian);
    let section_off = file.elf_header().e_shoff.get(NativeEndian);

    let mut offset = section_off as usize;

    for _ in 0..section_num {
        let sh_type = read_u32(bytes, offset + 4);
        let sh_flag = read_u64(bytes, offset + 8);

        // Attempt to determine whether this is the `.text` section without having to
        // lookup the string table.
        let is_text_section = sh_type == TEXT_SECTION_TYPE && sh_flag == TEXT_SECTION_FLAGS;

        // For the `.text` section:
        //   - set `sh_addr` to the in-memory location of the compiled functions,
        //   - set `sh_size` to the size of the compiled region in bytes.
        if is_text_section {
            // sh_addr
            write_u64(bytes, offset + 16, code_start.addr() as u64);

            // sh_size
            write_u64(bytes, offset + 32, code_size as u64);
        }
        // For all non-`.text` sections:
        //   - set `sh_addr` to the in-memory location of the ELF binary,
        //   - set `sh_flag` to `SHF_ALLOC` so debuggers will load them into memory.
        //
        // Skip the NULL section
        else if sh_type != 0 {
            let sh_offset = read_u64(bytes, offset + 24);
            let sh_abs_addr = unsafe { bytes.as_ptr().byte_add(sh_offset as usize) };

            // sh_flag
            write_u64(bytes, offset + 8, object::elf::SHF_ALLOC as u64);

            // sh_addr
            write_u64(bytes, offset + 16, sh_abs_addr.addr() as u64);
        }

        offset += SECTION_HEADER_SIZE;
    }

    Ok(())
}
