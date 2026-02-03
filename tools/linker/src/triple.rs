use lume_errors::{Result, diagnostic};

use crate::ObjectFormat;

/// Attempts to guess the target triple of the currently running system.
pub fn current_target_triple() -> TargetTriple {
    let arch = Arch::try_from(std::env::consts::ARCH).unwrap();
    let sys = Sys::try_from(std::env::consts::OS).unwrap();

    let env = match (arch, sys) {
        (_, Sys::Win32) => Env::Gnu,
        (_, Sys::Darwin) => Env::Macho,
        (_, _) => Env::Elf,
    };

    TargetTriple { arch, sys, env }
}

/// Attempts to parse a target triple from a string.
pub fn parse_target_triple<S: AsRef<str>>(value: S) -> Result<TargetTriple> {
    TargetTriple::try_from(value.as_ref())
        .map_err(|reason| diagnostic!("failed to parse target triple: {reason}").into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetTriple {
    pub arch: Arch,
    pub sys: Sys,
    pub env: Env,
}

impl TargetTriple {
    #[inline]
    pub fn object_format(self) -> ObjectFormat {
        match self.env {
            Env::Macho => ObjectFormat::MachO,
            Env::Elf | Env::Gnu => ObjectFormat::Elf,
        }
    }
}

impl TryFrom<&str> for TargetTriple {
    type Error = &'static str;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        let mut format_iter = value.split('-');
        let arch = format_iter.next().ok_or("missing architecture")?;
        let sys = format_iter.next().ok_or("missing system")?;
        let env = format_iter.next().ok_or("missing environment")?;

        let arch = Arch::try_from(arch)?;
        let sys = Sys::try_from(sys)?;
        let env = Env::try_from(env)?;

        Ok(TargetTriple { arch, sys, env })
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    #[cfg_attr(target_arch = "x86", default)]
    X86,

    #[cfg_attr(target_arch = "x86_64", default)]
    X86_64,

    #[cfg_attr(target_arch = "arm", default)]
    Arm,

    #[cfg_attr(target_arch = "aarch64", default)]
    Arm64,
}

impl Arch {
    pub fn is_64bit(self) -> bool {
        matches!(self, Arch::X86_64 | Arch::Arm64)
    }

    pub fn is_x86(self) -> bool {
        matches!(self, Arch::X86 | Arch::X86_64)
    }

    pub fn is_arm(self) -> bool {
        matches!(self, Arch::Arm | Arch::Arm64)
    }
}

impl TryFrom<&str> for Arch {
    type Error = &'static str;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "i386" | "x86" => Ok(Arch::X86),
            "x86_64" => Ok(Arch::X86_64),
            "arm" => Ok(Arch::Arm),
            "aarch64" | "arm64" => Ok(Arch::Arm64),
            _ => Err("unknown architecture given"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sys {
    None,
    Linux,
    Win32,
    Darwin,
}

impl TryFrom<&str> for Sys {
    type Error = &'static str;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "none" => Ok(Sys::None),
            "linux" => Ok(Sys::Linux),
            "win32" | "windows" => Ok(Sys::Win32),
            "macos" | "darwin" => Ok(Sys::Darwin),
            _ => Err("unknown system given"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Env {
    Gnu,
    Macho,
    Elf,
}

impl TryFrom<&str> for Env {
    type Error = &'static str;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value {
            "gnu" => Ok(Env::Gnu),
            "macho" => Ok(Env::Macho),
            "elf" => Ok(Env::Elf),
            _ => Err("unknown environment given"),
        }
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "constructed per host arch")]
pub enum Endianess {
    #[cfg_attr(target_endian = "big", default)]
    Big,

    #[cfg_attr(target_endian = "little", default)]
    Little,
}
