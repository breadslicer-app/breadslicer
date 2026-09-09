/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What a material's printer references resolve to.
//!
//! DragonFruit names a printer by preset id, and ships one preset per
//! machine and output container, so `elegoo-saturn-4-goo` and
//! `elegoo-saturn-4-ctb` are the same machine. Our ids name the machine
//! alone. A material record points at presets, so mapping one needs the
//! printer records of the same run to say which machine each preset is.

use std::collections::BTreeMap;

use sc_print_primitives::PrinterId;

/// The machines the run's printer records describe, by the preset id
/// each record carries.
#[derive(Debug, Default)]
pub(crate) struct PresetIndex {
    by_preset: BTreeMap<String, PrinterId>,
}

impl PresetIndex {
    /// Record that `preset` names `printer`.
    ///
    /// Several presets name one machine, one per container it writes, so
    /// entries agree rather than being unique, and the last one wins.
    pub(crate) fn insert(&mut self, preset: &str, printer: PrinterId) {
        self.by_preset.insert(preset.to_owned(), printer);
    }

    /// Find the machine `preset` names, or `None` when no printer record
    /// of this run carries that preset id.
    pub(crate) fn printer(&self, preset: &str) -> Option<&PrinterId> {
        self.by_preset.get(preset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printer(id: &str) -> PrinterId {
        PrinterId::new(id).unwrap()
    }

    #[test]
    fn a_preset_resolves_to_the_machine_it_was_recorded_with() {
        let mut index = PresetIndex::default();
        index.insert("elegoo-saturn-4-goo", printer("elegoo-saturn-4"));

        assert_eq!(
            index.printer("elegoo-saturn-4-goo"),
            Some(&printer("elegoo-saturn-4")),
        );
    }

    #[test]
    fn the_containers_of_one_machine_resolve_to_the_same_id() {
        let mut index = PresetIndex::default();
        index.insert("elegoo-mars-3-ctb", printer("elegoo-mars-3"));
        index.insert("elegoo-mars-3-goo", printer("elegoo-mars-3"));

        assert_eq!(
            index.printer("elegoo-mars-3-ctb"),
            index.printer("elegoo-mars-3-goo"),
        );
    }

    #[test]
    fn an_unrecorded_preset_resolves_to_nothing() {
        let mut index = PresetIndex::default();
        index.insert("elegoo-saturn-4-goo", printer("elegoo-saturn-4"));

        assert_eq!(index.printer("elegoo-saturn-4-ctb"), None);
        assert_eq!(index.printer(""), None);
    }
}
