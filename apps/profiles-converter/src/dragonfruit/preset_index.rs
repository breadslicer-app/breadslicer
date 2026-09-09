/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What a material's printer references resolve to.
//!
//! DragonFruit names a printer by preset id, and ships one preset per
//! machine and output container, so `elegoo-saturn-4-goo` and
//! `elegoo-saturn-4-ctb` are the same machine writing 2 containers. Our
//! ids name the machine alone. A material record points at presets and
//! keeps its settings per container, so mapping one needs the printer
//! records of the same run to say which machine each preset is and which
//! container it writes.

use std::collections::BTreeMap;

use sc_print_primitives::PrinterId;

/// What one preset id names: the machine, under our id, and the
/// container that preset writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Preset {
    pub printer: PrinterId,
    pub container: String,
}

/// The presets the run's printer records carry.
#[derive(Debug, Default)]
pub(crate) struct PresetIndex {
    by_preset: BTreeMap<String, Preset>,
}

impl PresetIndex {
    /// Record what `preset` names.
    ///
    /// Several presets name one machine, one per container it writes, so
    /// entries agree on the machine rather than being unique, and the
    /// last one wins.
    pub(crate) fn insert(&mut self, preset: &str, entry: Preset) {
        self.by_preset.insert(preset.to_owned(), entry);
    }

    /// Find what `preset` names, or `None` when no printer record of this
    /// run carries that preset id.
    pub(crate) fn get(&self, preset: &str) -> Option<&Preset> {
        self.by_preset.get(preset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(id: &str, container: &str) -> Preset {
        Preset {
            printer: PrinterId::new(id).unwrap(),
            container: container.to_owned(),
        }
    }

    #[test]
    fn a_preset_resolves_to_the_machine_and_container_it_was_recorded_with() {
        let mut index = PresetIndex::default();
        index.insert("elegoo-saturn-4-goo", preset("elegoo-saturn-4", ".goo"));

        assert_eq!(
            index.get("elegoo-saturn-4-goo"),
            Some(&preset("elegoo-saturn-4", ".goo")),
        );
    }

    #[test]
    fn the_presets_of_one_machine_share_its_id_and_differ_in_container() {
        let mut index = PresetIndex::default();
        index.insert("elegoo-mars-3-ctb", preset("elegoo-mars-3", ".ctb"));
        index.insert("elegoo-mars-3-goo", preset("elegoo-mars-3", ".goo"));

        let ctb = index.get("elegoo-mars-3-ctb").unwrap();
        let goo = index.get("elegoo-mars-3-goo").unwrap();
        assert_eq!(ctb.printer, goo.printer);
        assert_eq!(
            (ctb.container.as_str(), goo.container.as_str()),
            (".ctb", ".goo")
        );
    }

    #[test]
    fn an_unrecorded_preset_resolves_to_nothing() {
        let mut index = PresetIndex::default();
        index.insert("elegoo-saturn-4-goo", preset("elegoo-saturn-4", ".goo"));

        assert_eq!(index.get("elegoo-saturn-4-ctb"), None);
        assert_eq!(index.get(""), None);
    }
}
