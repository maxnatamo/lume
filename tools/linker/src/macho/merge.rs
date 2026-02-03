use lume_span::Internable;

use crate::*;

/// Merge all sections with the same section names into single sections.
pub fn merge_sections(db: &mut Database) {
    let mut segments = IndexMap::<String, IndexSet<OutputSectionId>>::new();
    let mut sections = IndexMap::<OutputSectionId, OutputSection>::new();

    for input_section in db.input_sections() {
        let output_section_id = OutputSectionId::from_name(input_section.segment.as_deref(), &input_section.name);

        let segment_name = input_section.segment.clone().unwrap_or_default();
        segments.entry(segment_name).or_default().insert(output_section_id);

        let output_section = sections.entry(output_section_id).or_insert_with(|| OutputSection {
            id: output_section_id,
            name: SectionName {
                segment: input_section.segment.clone().map(|str| str.intern()),
                section: input_section.name.intern(),
            },
            placement: input_section.placement,
            size: 0,
            alignment: 1,
            kind: input_section.kind,
            flags: input_section.flags,
            merged_from: IndexSet::new(),
        });

        output_section.size += input_section.data.len() as u64;
        output_section.alignment = output_section.alignment.max(input_section.alignment);
        output_section.flags |= input_section.flags;
        output_section.merged_from.insert(input_section.id);
    }

    db.output_segments = segments;
    db.output_sections = sections;
}
