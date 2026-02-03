use lume_errors::Result;

use crate::write::Writer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ElfClass {
    Elf32,
    Elf64,
}

pub(crate) trait WritePod {
    fn write_to<W: Writer>(self, class: ElfClass, writer: &mut W) -> Result<()>;
}

macro_rules! declare_pod {
    (
        $(#[$outer:meta])*
        $type_vis:vis struct $type_name:ident {
            $(
                $(#[$inner:ident $($args:tt)*])*
                $field_vis:vis $field_name:ident: Field<$field_t32:ty, $field_t64:ty>,
            )*
        }
    ) => {
        $(#[$outer])*
        $type_vis struct $type_name {
            $(
                $(#[$inner $($args)*])*
                $field_vis $field_name: $field_t64,
            )*
        }

        impl WritePod for $type_name {
            fn write_to<W: Writer>(self, class: ElfClass, writer: &mut W) -> Result<()> {
                match class {
                    ElfClass::Elf32 => {
                        $(
                            writer.write(
                                &<$field_t32>::to_ne_bytes(
                                    <$field_t32>::try_from(self.$field_name).unwrap()
                                )
                            )?;
                        )*
                    }
                    ElfClass::Elf64 => {
                        $(
                            writer.write(&<$field_t64>::to_ne_bytes(self.$field_name))?;
                        )*
                    }
                }

                Ok(())
            }
        }
    };
}

pub(crate) struct Output {
    pub ehdr: FileHeader,
    pub phdrs: Vec<ProgramHeaderEntry>,
    pub shdrs: Vec<SectionHeaderTableEntry>,
}

#[allow(clippy::struct_field_names)]
pub(crate) struct FileHeader {
    pub e_machine: u16,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

declare_pod! {
    #[allow(clippy::struct_field_names)]
    pub(crate) struct ProgramHeaderEntry {
        pub p_type: Field<u32, u32>,
        pub p_offset: Field<u32, u64>,
        pub p_vaddr: Field<u32, u64>,
        pub p_paddr: Field<u32, u64>,
        pub p_filesz: Field<u32, u64>,
        pub p_memsz: Field<u32, u64>,
        pub p_flags: Field<u32, u32>,
        pub p_align: Field<u32, u64>,
    }
}

declare_pod! {
    #[allow(clippy::struct_field_names)]
    pub(crate) struct SectionHeaderTableEntry {
        pub sh_name: Field<u32, u32>,
        pub sh_type: Field<u32, u32>,
        pub sh_flags: Field<u32, u64>,
        pub sh_addr: Field<u32, u64>,
        pub sh_offset: Field<u32, u64>,
        pub sh_size: Field<u32, u64>,
        pub sh_link: Field<u32, u32>,
        pub sh_info: Field<u32, u32>,
        pub sh_addralign: Field<u32, u64>,
        pub sh_entsize: Field<u32, u64>,
    }
}
