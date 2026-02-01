use crate::RelocationTarget;
use crate::macho::layout::Layout;

impl Layout<'_> {
    pub(crate) fn apply_relocations(&mut self) {
        let output_section_ids = self.ctx.db.output_sections().map(|sec| sec.id).collect::<Vec<_>>();

        for output_section_id in output_section_ids {
            let input_section_ids = self.ctx.db.output_section(output_section_id).merged_from.clone();

            for input_section_id in input_section_ids {
                let section = self.ctx.db.input_section(input_section_id);
                let relocations = section.relocations.clone();

                for relocation in relocations {
                    let reloc_offset = usize::try_from(relocation.address).unwrap();
                    let target_address = match relocation.target {
                        RelocationTarget::Absolute => relocation.address,
                        RelocationTarget::Symbol(symbol_id) => self.vmaddr_of_symbol(symbol_id),
                        RelocationTarget::InputSection(section_id) => self.vmaddr_of_input_section(section_id),
                        RelocationTarget::OutputSection(section_id) => self.vmaddr_of_output_section(section_id),
                    };

                    let target_address = target_address.checked_add_signed(relocation.addend).unwrap_or_else(|| {
                        panic!(
                            "could not calculate relocation target address: 0x{target_address:016x} + {}",
                            relocation.addend
                        )
                    });

                    let section = self.ctx.db.input_section_mut(input_section_id);
                    let target_address_bytes = target_address.to_ne_bytes();

                    section.data[reloc_offset..reloc_offset + relocation.length as usize]
                        .copy_from_slice(&target_address_bytes[..relocation.length as usize]);
                }
            }
        }
    }
}
