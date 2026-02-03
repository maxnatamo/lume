use std::sync::LazyLock;

use regex::Regex;

use crate::elf::layout::{SegmentFlags, SegmentType};
use crate::*;

#[derive(Debug, Clone)]
pub(crate) struct SegmentRules(Vec<SegmentRule>);

impl SegmentRules {
    pub fn new<I>(rules: I) -> Self
    where
        I: IntoIterator<Item = SegmentRule>,
    {
        Self(rules.into_iter().collect())
    }

    pub fn matching_segment_of(&self, section: &OutputSection) -> Option<&SegmentRule> {
        self.0
            .iter()
            .find(|rule| rule.matcher.iter().any(|matcher| matcher.matches(section)))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SegmentRule {
    /// Type of the segment
    pub segment_type: SegmentType,

    /// Flags for the segment
    pub segment_flags: SegmentFlags,

    /// Predicate for the rule to match
    pub matcher: Vec<SectionMatcher>,
}

pub(crate) static SEGMENT_RULES: LazyLock<SegmentRules> = LazyLock::new(|| {
    SegmentRules::new(vec![
        SegmentRule {
            matcher: Vec::new(),
            segment_type: SegmentType::PHDR,
            segment_flags: SegmentFlags::R,
        },
        SegmentRule {
            matcher: vec![
                SectionMatcher::name_equals(".init"),
                SectionMatcher::name_equals(".text"),
                SectionMatcher::name_equals(".fini"),
                SectionMatcher::name_equals(".rela.dyn"),
                SectionMatcher::name_equals(".rela.plt"),
                SectionMatcher::name_equals(".plt"),
            ],
            segment_type: SegmentType::LOAD,
            segment_flags: SegmentFlags::RX,
        },
        SegmentRule {
            matcher: vec![
                SectionMatcher::name_equals(".init_array"),
                SectionMatcher::name_equals(".fini_array"),
                SectionMatcher::name_equals(".dynamic"),
                SectionMatcher::name_equals(".data"),
                SectionMatcher::name_equals(".bss"),
            ],
            segment_type: SegmentType::LOAD,
            segment_flags: SegmentFlags::RW,
        },
    ])
});

#[derive(Debug, Clone)]
pub(crate) struct SectionRules(Vec<SectionRule>);

impl SectionRules {
    pub fn new<I>(rules: I) -> Self
    where
        I: IntoIterator<Item = SectionRule>,
    {
        Self(rules.into_iter().collect())
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &SectionRule> {
        self.0.iter()
    }

    pub fn outcome_of(&self, section: &OutputSection) -> Option<SectionOutcome> {
        self.iter().find_map(|rule| {
            if rule.matcher.matches(section) {
                Some(rule.outcome.clone())
            } else {
                None
            }
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SectionRule {
    /// Predicate for the rule to match
    pub matcher: SectionMatcher,

    /// Outcome for the section, if the rule matches
    pub outcome: SectionOutcome,
}

#[derive(Debug, Clone)]
pub(crate) enum SectionMatcher {
    /// Name must match the given string, case insensitive.
    Name(&'static str),

    /// Name must match the given regex pattern.
    NamePattern(Regex),

    /// Section kind must match the given kind.
    KindOf(SectionKind),
}

impl SectionMatcher {
    /// Name must match the given string, case insensitive.
    pub fn name_equals(name: &'static str) -> Self {
        Self::Name(name)
    }

    /// Name must match the given regex pattern.
    pub fn name_matches<S: TryInto<Regex>>(pattern: S) -> Option<Self> {
        pattern.try_into().ok().map(Self::NamePattern)
    }

    /// Section kind must match the given kind.
    pub fn kind_of(kind: SectionKind) -> Self {
        Self::KindOf(kind)
    }

    pub fn matches(&self, section: &OutputSection) -> bool {
        match self {
            SectionMatcher::Name(name) => name.eq_ignore_ascii_case(&section.name.section),
            SectionMatcher::NamePattern(pattern) => pattern.is_match(&section.name.section),
            SectionMatcher::KindOf(kind) => section.kind == *kind,
        }
    }
}

#[derive(Default, Debug, Clone)]
pub(crate) enum SectionOutcome {
    /// Ignore the section and leave it as-is.
    #[default]
    Ignore,

    /// The section must be present and have the given flags.
    ///
    /// This is only available when using the [`SectionMatcher::Name`] matcher.
    Required { kind: SectionKind, flags: SectionFlags },

    /// The section must not be present.
    Discard,
}

pub(crate) static SECTION_RULES: LazyLock<SectionRules> = LazyLock::new(|| {
    SectionRules::new(vec![
        SectionRule {
            matcher: SectionMatcher::name_equals(".init"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::Text,
                flags: SectionFlags::Allocate | SectionFlags::Executable,
            },
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".text"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::Text,
                flags: SectionFlags::Allocate | SectionFlags::Executable,
            },
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".fini"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::Text,
                flags: SectionFlags::Allocate | SectionFlags::Executable,
            },
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".preinit_array"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::Elf(object::elf::SHT_PREINIT_ARRAY),
                flags: SectionFlags::Allocate | SectionFlags::Writable,
            },
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".init_array"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::Elf(object::elf::SHT_INIT_ARRAY),
                flags: SectionFlags::Allocate | SectionFlags::Writable,
            },
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".fini_array"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::Elf(object::elf::DT_FINI_ARRAY),
                flags: SectionFlags::Allocate | SectionFlags::Writable,
            },
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".rodata"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::ReadOnlyData,
                flags: SectionFlags::Allocate | SectionFlags::Merge,
            },
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".data"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::Data,
                flags: SectionFlags::Allocate | SectionFlags::Writable,
            },
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".tdata"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::Data,
                flags: SectionFlags::Allocate | SectionFlags::Writable | SectionFlags::TLS,
            },
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".bss"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::UninitializedData,
                flags: SectionFlags::Allocate | SectionFlags::Writable,
            },
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".tbss"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::UninitializedData,
                flags: SectionFlags::Allocate | SectionFlags::Writable | SectionFlags::TLS,
            },
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".rela.iplt"),
            outcome: SectionOutcome::Required {
                kind: SectionKind::Elf(object::elf::SHT_RELA),
                flags: SectionFlags::Allocate,
            },
        },
        SectionRule {
            matcher: SectionMatcher::kind_of(SectionKind::StringTable),
            outcome: SectionOutcome::Discard,
        },
        SectionRule {
            matcher: SectionMatcher::kind_of(SectionKind::Elf(object::elf::SHT_SYMTAB)),
            outcome: SectionOutcome::Discard,
        },
        SectionRule {
            matcher: SectionMatcher::kind_of(SectionKind::Elf(object::elf::SHT_GROUP)),
            outcome: SectionOutcome::Discard,
        },
        SectionRule {
            matcher: SectionMatcher::kind_of(SectionKind::Elf(object::elf::SHT_STRTAB)),
            outcome: SectionOutcome::Discard,
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".interp"),
            outcome: SectionOutcome::Discard,
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".shstrtab"),
            outcome: SectionOutcome::Discard,
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".rela"),
            outcome: SectionOutcome::Discard,
        },
        SectionRule {
            matcher: SectionMatcher::name_equals(".crel"),
            outcome: SectionOutcome::Discard,
        },
        SectionRule {
            matcher: SectionMatcher::name_matches(r"\.note\..+").expect("regex pattern to be valid"),
            outcome: SectionOutcome::Discard,
        },
        SectionRule {
            matcher: SectionMatcher::name_matches(r"\.gnu\.glibc.+").expect("regex pattern to be valid"),
            outcome: SectionOutcome::Discard,
        },
        SectionRule {
            matcher: SectionMatcher::name_matches(r"\.gnu\.warning.+").expect("regex pattern to be valid"),
            outcome: SectionOutcome::Discard,
        },
    ])
});

pub(crate) fn apply_rules(db: &mut Database) {
    for output_section_id in db.output_sections.keys().copied().collect::<Vec<_>>() {
        let output_section = db.output_section_mut(output_section_id);
        let section_outcome = SECTION_RULES.outcome_of(output_section).unwrap_or_default();

        match section_outcome {
            SectionOutcome::Ignore => {}
            SectionOutcome::Discard => {
                db.output_sections.swap_remove(&output_section_id);

                for output_segment in db.output_segments.values_mut() {
                    output_segment.shift_remove(&output_section_id);
                }

                continue;
            }
            SectionOutcome::Required { kind, flags } => {
                output_section.kind = kind;
                output_section.flags = flags;
            }
        }

        let matching_segment_definition = match SEGMENT_RULES.matching_segment_of(output_section) {
            Some(segment) => segment.clone(),
            None => SegmentRule {
                segment_type: SegmentType::LOAD,
                segment_flags: {
                    let mut flags = SegmentFlags::empty();

                    if output_section.flags.contains(SectionFlags::Readable) {
                        flags.insert(SegmentFlags::R);
                    }

                    if output_section.flags.contains(SectionFlags::Writable) {
                        flags.insert(SegmentFlags::W);
                    }

                    if output_section.flags.contains(SectionFlags::Executable) {
                        flags.insert(SegmentFlags::X);
                    }

                    flags
                },
                matcher: Vec::new(),
            },
        };
    }
}
