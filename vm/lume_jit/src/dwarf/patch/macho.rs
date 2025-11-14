use lume_errors::Result;

const TEXT_SECTION_NAME: &str = "__text\0\0\0\0\0\0\0\0\0\0";
const TEXT_SEGMENT_NAME: &str = "__TEXT\0\0\0\0\0\0\0\0\0\0";

/// `object` restricts which attributes can be defined as a custom value, so we
/// manually patch the MachO binary.
///
/// This operation MUST be done in-memory and without copying the file content.
pub(crate) fn patch_binary_file(bytes: &mut [u8], code_start: *const u8, code_size: usize) -> Result<()> {
    let mut patch = super::Patch::new(bytes);

    assert_eq!(patch.read_u32(0), object::macho::MH_MAGIC_64);

    // cputype + cpusubtype + filetype
    patch.skip(12);

    let ncmds = patch.read_u32(0);

    // cmdsize + flags + reserved
    patch.skip(12);

    for _ in 0..ncmds {
        let cmd = patch.read_u32(0);
        let cmdsize = patch.read_u32(0);

        if cmd == object::macho::LC_SEGMENT_64 {
            // sectname + vmaddr + vmsize
            patch.skip(32);

            // fileoff + filesize + maxprot + initprot
            patch.skip(24);

            let nsects = patch.read_u32(0);

            // flags
            patch.skip(4);

            for _ in 0..nsects {
                let sectname = String::from_utf8_lossy(patch.read_n::<16>(0)).to_string();
                let segname = String::from_utf8_lossy(patch.read_n::<16>(0)).to_string();

                // We're only checking the start of the strings, since they both contain padding
                // enough for all 16 bytes.
                let is_text_section = sectname == TEXT_SECTION_NAME && segname == TEXT_SEGMENT_NAME;

                // For the `__text` section:
                //   - set `addr` to the in-memory location of the compiled functions,
                //   - set `size` to the size of the compiled region in bytes.
                if is_text_section {
                    // addr
                    patch.write_u64(0, code_start.addr() as u64);

                    // size
                    patch.write_u64(0, code_size as u64);

                    // offset
                    patch.skip(4);
                } else {
                    let offset = patch.peek_u32(16);
                    let abs_addr = unsafe { patch.bytes.as_ptr().byte_add(offset as usize) };

                    // addr
                    patch.write_u64(0, abs_addr.addr() as u64);

                    // size + offset
                    patch.skip(12);
                }

                // offset + align + reloff + nreloc + flags + reserved*
                patch.skip(28);
            }
        } else if cmd == object::macho::LC_SYMTAB {
            let symoff = patch.read_u32(0);
            let nsyms = patch.read_u32(0);

            // stroff + strsize
            patch.skip(8);

            let prev_offset = patch.offset;
            patch.offset = symoff as usize;

            for _ in 0..nsyms {
                // n_strx
                patch.skip(4);

                let n_type = patch.read_u8(0);

                // n_sect + n_desc
                patch.skip(3);

                // Skip symbols outside any sections
                if n_type & object::macho::N_TYPE != object::macho::N_SECT {
                    // n_value
                    patch.skip(8);

                    continue;
                }

                let offset = patch.peek_u64(0);
                let abs_addr = unsafe { code_start.byte_add(offset as usize) };

                patch.write_u64(0, abs_addr.addr() as u64);
            }

            patch.offset = prev_offset;
        } else {
            patch.skip((cmdsize - 8).cast_signed() as _);
        }
    }

    Ok(())
}
