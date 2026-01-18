use crate::{CustomEntry, Layout, RelocationTarget};

impl<C: CustomEntry> Layout<'_, C> {
    pub(crate) fn apply_relocations(&mut self) {
        let merged_section_ids = self.db.merged_sections().map(|sec| sec.id).collect::<Vec<_>>();

        for merged_section_id in merged_section_ids {
            let section_ids = self.db.merged_section(merged_section_id).merged_from.clone();

            for section_id in section_ids {
                let section = self.db.section(section_id);
                let relocations = section.relocations.clone();

                for relocation in relocations {
                    let reloc_offset = usize::try_from(relocation.address).unwrap();
                    let target_address = match relocation.target {
                        RelocationTarget::Absolute => relocation.address,
                        RelocationTarget::Symbol(symbol_id) => self.vaddr_of_symbol(symbol_id),
                        RelocationTarget::Section(section_id) => self.vaddr_of_unmerged_section(section_id),
                    };

                    let target_address = target_address.checked_add_signed(relocation.addend).unwrap_or_else(|| {
                        panic!(
                            "could not calculate relocation target address: 0x{target_address:016x} + {}",
                            relocation.addend
                        )
                    });

                    let section = self.db.section_mut(section_id);
                    let target_address_bytes = target_address.to_ne_bytes();

                    println!(
                        "[{}] apply reloc at {reloc_offset}, {} bytes => 0x{target_address:016x}",
                        section.name, relocation.length,
                    );

                    section.data[reloc_offset..reloc_offset + relocation.length as usize]
                        .copy_from_slice(&target_address_bytes[..relocation.length as usize]);
                }
            }
        }
    }
}
