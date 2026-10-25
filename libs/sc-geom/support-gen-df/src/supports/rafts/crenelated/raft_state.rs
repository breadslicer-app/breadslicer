//! DragonFruit `src/supports/Rafts/Crenelated/RaftState.ts`: the raft settings
//! store, without its listeners and session flag (the GUI's). Every write goes
//! through the store's resolution: the footprint border margin is clamped to
//! 0.05 mm, and a base that is not solid has no wall (`wallEnabled` is forced
//! off, while the last solid choice is remembered for the way back).

use std::collections::HashMap;

use super::raft_types::{DEFAULT_RAFT_SETTINGS, RaftBottomMode, RaftSettings};
use crate::three::math_utils::{js_max, js_min};

const FOOTPRINT_BORDER_MARGIN_MAX_MM: f64 = 0.05;

/// `normalizeRaftSettings`.
pub fn normalize_raft_settings(settings: RaftSettings) -> RaftSettings {
    RaftSettings {
        footprint_border_margin: js_min(
            FOOTPRINT_BORDER_MARGIN_MAX_MM,
            js_max(0.0, settings.footprint_border_margin),
        ),
        ..settings
    }
}

/// The raft settings store: the global settings and per-model overrides.
/// The fields hold resolved settings; write through the methods to resolve.
#[derive(Debug, Clone)]
pub struct RaftSettingsStore {
    pub global: RaftSettings,
    pub per_model: HashMap<String, RaftSettings>,
    /// `rememberedSolidWallEnabled`.
    pub remembered_solid_wall_enabled: bool,
}

impl Default for RaftSettingsStore {
    fn default() -> Self {
        RaftSettingsStore {
            global: DEFAULT_RAFT_SETTINGS,
            per_model: HashMap::new(),
            remembered_solid_wall_enabled: DEFAULT_RAFT_SETTINGS.wall_enabled,
        }
    }
}

impl RaftSettingsStore {
    /// `resolveRaftSettings(next, wallIsExplicit)`.
    fn resolve(&mut self, next: RaftSettings, wall_is_explicit: bool) -> RaftSettings {
        let normalized = normalize_raft_settings(next);
        if normalized.bottom_mode != RaftBottomMode::Solid {
            return RaftSettings {
                wall_enabled: false,
                ..normalized
            };
        }
        let wall_enabled = if wall_is_explicit {
            normalized.wall_enabled
        } else {
            self.remembered_solid_wall_enabled
        };
        self.remembered_solid_wall_enabled = wall_enabled;
        RaftSettings {
            wall_enabled,
            ..normalized
        }
    }

    /// `setRaftSettings(settings)` (and `applyImportDefaultRaftSettings`).
    pub fn set_global(&mut self, settings: RaftSettings) {
        self.global = self.resolve(settings, true);
    }

    /// `updateRaftSettings(partial)`: `patch` edits a copy of the current
    /// settings; `sets_wall` says whether the patch names `wallEnabled` (a mode
    /// switch does not, and gets the last solid choice back).
    pub fn update_global(&mut self, patch: impl FnOnce(&mut RaftSettings), sets_wall: bool) {
        let mut next = self.global;
        patch(&mut next);
        self.global = self.resolve(next, sets_wall);
    }

    /// `setRaftSettingsForModel(modelId, settings)`.
    pub fn set_for_model(&mut self, model_id: &str, settings: RaftSettings) {
        let normalized = normalize_raft_settings(settings);
        let wall_enabled =
            normalized.bottom_mode == RaftBottomMode::Solid && normalized.wall_enabled;
        self.per_model.insert(
            model_id.to_owned(),
            RaftSettings {
                wall_enabled,
                ..normalized
            },
        );
    }

    /// `clearRaftSettingsForModel(modelId)`.
    pub fn clear_for_model(&mut self, model_id: &str) {
        self.per_model.remove(model_id);
    }

    /// `getRaftSettingsForModel(modelId)`.
    pub fn for_model(&self, model_id: Option<&str>) -> RaftSettings {
        match model_id {
            Some(id) if !id.is_empty() => self.per_model.get(id).copied().unwrap_or(self.global),
            _ => self.global,
        }
    }
}
