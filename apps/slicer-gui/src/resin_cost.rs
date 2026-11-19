/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What a print costs in resin (#101, #45): the user's price for each resin
//! (bottle price and content, by volume or by weight, and the losses a print
//! has on top of what it cures), and the cost of the held slice, print and
//! model by model, in the Slice tab.
//!
//! The price lives in the user's settings, not in the profiles: it changes
//! with the country and the shop. The cost is what the print takes out of
//! the bottle: the resin the slice cures, the losses added, at the bottle's
//! price per ml (or per gram, when it is sold by weight, through the resin's
//! liquid density). Each model's share is the part of that resin its own
//! volume and its supports' make of the plate's; the shares add up to the
//! print.

use std::cell::RefCell;
use std::rc::Rc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::SceneSpatial;
use sc_print_primitives::MaterialId;
use sc_scene::NodeId;
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::MainWindow;
use crate::i18n::ctr;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::scene_document::SceneDocument;
use crate::viewport::ViewportController;

/// What a bottle's content is counted in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BottleUnit {
    /// Millilitres: the price is per volume.
    #[default]
    Ml,
    /// Grams: the price is per weight, liquid.
    G,
}

fn is_ml(unit: &BottleUnit) -> bool {
    *unit == BottleUnit::Ml
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

/// What the user pays for a resin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ResinPrice {
    /// What a bottle costs.
    pub(crate) price: f64,
    /// The currency it is written in, as the user writes it ("$", "€").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) currency: String,
    /// What a bottle holds, in [`unit`](Self::unit).
    pub(crate) amount: f64,
    #[serde(default, skip_serializing_if = "is_ml")]
    pub(crate) unit: BottleUnit,
    /// What a print takes out of the bottle on top of what it cures (resin
    /// left on the part and the plate, wiped off, spilled), as a share of
    /// what it cures, %.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) losses_pct: f64,
}

impl ResinPrice {
    /// What a print that cures `volume_ml` of resin, weighing `liquid_g`
    /// liquid, costs: `None` when the bottle is sold by weight and the resin
    /// gives no density.
    pub(crate) fn cost_of(&self, volume_ml: f64, liquid_g: Option<f64>) -> Option<f64> {
        let used = match self.unit {
            BottleUnit::Ml => volume_ml,
            BottleUnit::G => liquid_g?,
        };
        (self.price > 0.0 && self.amount > 0.0)
            .then(|| used * (1.0 + self.losses_pct / 100.0) * self.price / self.amount)
    }
}

/// `3.4`, `3,4` or ` 3.40 ` as a number: `None` for anything else.
pub(crate) fn parse_number(text: &str) -> Option<f64> {
    crate::numbers::parse_number(text).filter(|v| v.is_finite())
}

/// `$ 3.40`, or `3.40` with no currency.
pub(crate) fn money(currency: &str, value: f64) -> String {
    let currency = currency.trim();
    let value = crate::numbers::shown(&format!("{value:.2}"));
    if currency.is_empty() {
        value
    } else {
        format!("{currency} {value}")
    }
}

/// `mesh` as `world` places it: the volume it encloses, mm³ (what an open
/// one's faces leave unbalanced counts too; it only weighs a share).
fn world_volume_mm3(mesh: &IndexedMesh, world: &Transform3D) -> f64 {
    let points: Vec<[f64; 3]> = mesh
        .vertices()
        .iter()
        .map(|v| world.apply_to_vertex(*v).coords().map(f64::from))
        .collect();
    let sum: f64 = mesh
        .faces()
        .iter()
        .map(|face| {
            let [a, b, c] = face.vertices.map(|i| points[usize::from(i)]);
            a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0])
        })
        .sum();
    (sum / 6.0).abs()
}

/// Each model of `spatial` that `on` says is sliced, with the share of the
/// plate's resin it and its supports take (`model_of` gives the model a
/// support leaf belongs to): in scene order, the shares adding up to 1.
pub(crate) fn model_shares(
    spatial: &SceneSpatial,
    on: &dyn Fn(NodeId) -> bool,
    model_of: &dyn Fn(NodeId) -> Option<NodeId>,
) -> Vec<(NodeId, f64)> {
    let mut volumes: Vec<(NodeId, f64)> = Vec::new();
    for leaf in (0..).map_while(|i| spatial.leaf_id_at_index(i)) {
        let Some(mesh) = spatial.graph().leaf_mesh_arc(leaf) else {
            continue;
        };
        if mesh.faces().is_empty() || !on(leaf) {
            continue;
        }
        let model = if sc_geom::df_supports::is_any_support_mesh(mesh) {
            match model_of(leaf) {
                Some(model) => model,
                None => continue,
            }
        } else {
            leaf
        };
        let Some(query) = spatial.leaf(leaf) else {
            continue;
        };
        let volume = world_volume_mm3(mesh, &query.world_transform());
        match volumes.iter_mut().find(|(id, _)| *id == model) {
            Some((_, total)) => *total += volume,
            None => volumes.push((model, volume)),
        }
    }
    let total: f64 = volumes.iter().map(|(_, v)| v).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    volumes.into_iter().map(|(id, v)| (id, v / total)).collect()
}

/// The fields of the price card, by the index its callbacks pass.
const PRICE: i32 = 0;
const CURRENCY: i32 = 1;
const AMOUNT: i32 = 2;
const LOSSES: i32 = 3;
const WEIGHED: i32 = 4;

/// The Slice tab's resin cost: the price card of the held slice's resin
/// and what the print, and each model, costs.
pub(crate) struct ResinCostPanel {
    ui: slint::Weak<MainWindow>,
    bus: Rc<PreferencesBus>,
    document: Rc<SceneDocument>,
    ctrl: Rc<ViewportController>,
    /// The resin the card shows, and its fields as typed.
    shown: RefCell<Option<MaterialId>>,
    drafts: RefCell<[String; 5]>,
    unit: RefCell<BottleUnit>,
    /// Each model of the held slice, named, with its share of the resin.
    shares: RefCell<Vec<(String, f64)>>,
    _subscription: RefCell<Option<Subscription>>,
}

impl ResinCostPanel {
    pub(crate) fn install(
        ui: &MainWindow,
        bus: Rc<PreferencesBus>,
        document: Rc<SceneDocument>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let panel = Rc::new(Self {
            ui: ui.as_weak(),
            bus: bus.clone(),
            document,
            ctrl: ctrl.clone(),
            shown: RefCell::new(None),
            drafts: RefCell::new(Default::default()),
            unit: RefCell::new(BottleUnit::Ml),
            shares: RefCell::new(Vec::new()),
            _subscription: RefCell::new(None),
        });
        let weak = Rc::downgrade(&panel);
        ctrl.on_estimate(move || {
            if let Some(panel) = weak.upgrade() {
                panel.slice_changed();
            }
        });
        let weak = Rc::downgrade(&panel);
        let subscription = bus.subscribe(move |_, change| {
            if *change == PreferencesChange::ResinPrices
                && let Some(panel) = weak.upgrade()
            {
                panel.show_cost();
            }
        });
        *panel._subscription.borrow_mut() = Some(subscription);
        let weak = Rc::downgrade(&panel);
        ui.on_slice_price_edited(move |field, text| {
            if let Some(panel) = weak.upgrade() {
                panel.edited(field, text.as_str());
            }
        });
        let weak = Rc::downgrade(&panel);
        ui.on_slice_price_unit_picked(move |at| {
            if let Some(panel) = weak.upgrade() {
                *panel.unit.borrow_mut() = if at == 1 {
                    BottleUnit::G
                } else {
                    BottleUnit::Ml
                };
                panel.store();
            }
        });
        panel.slice_changed();
        panel
    }

    #[cfg(test)]
    pub(crate) fn prices_for_test(&self) -> std::collections::BTreeMap<MaterialId, ResinPrice> {
        self.bus.snapshot().resin_prices
    }

    /// The held slice or its estimate changed: the card follows its resin,
    /// and the models' shares are worked out again.
    fn slice_changed(&self) {
        let resin = self.document.slice_params().map(|params| params.resin);
        if resin != *self.shown.borrow() {
            self.show_price_of(resin.as_ref());
            *self.shown.borrow_mut() = resin;
        }
        *self.shares.borrow_mut() = self.named_shares();
        self.show_cost();
    }

    /// The fields of `resin`'s price, as stored.
    fn show_price_of(&self, resin: Option<&MaterialId>) {
        let price = resin.and_then(|id| self.bus.snapshot().resin_prices.get(id).cloned());
        let tidy = |v: f64| {
            let text = format!("{v:.4}");
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        };
        let drafts = match &price {
            Some(p) => [
                tidy(p.price),
                p.currency.clone(),
                tidy(p.amount),
                if p.losses_pct == 0.0 {
                    String::new()
                } else {
                    tidy(p.losses_pct)
                },
                String::new(),
            ],
            None => Default::default(),
        };
        *self.unit.borrow_mut() = price.as_ref().map(|p| p.unit).unwrap_or_default();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_price_text(drafts[0].clone().into());
            ui.set_slice_price_currency(drafts[1].clone().into());
            ui.set_slice_price_amount(drafts[2].clone().into());
            ui.set_slice_price_losses(drafts[3].clone().into());
            ui.set_slice_price_weighed(SharedString::default());
            ui.set_slice_price_unit(i32::from(*self.unit.borrow() == BottleUnit::G));
        }
        *self.drafts.borrow_mut() = drafts;
    }

    fn named_shares(&self) -> Vec<(String, f64)> {
        let Some(params) = self.document.slice_params() else {
            return Vec::new();
        };
        let volume = self.ctrl.build_volume();
        let plates = self.document.plates();
        let sessions = self.document.support_sessions();
        let model_of = |leaf| crate::support_persist::model_of(&sessions, leaf);
        self.document
            .with_scene(|spatial| {
                let on = |leaf| {
                    plates.count() <= 1
                        || crate::plate_layout::plate_holding(
                            spatial, &volume, plates, leaf, &model_of,
                        ) == Some(params.plate)
                };
                let names = crate::repair_report::object_names_by_leaf(spatial.graph());
                model_shares(spatial, &on, &model_of)
                    .into_iter()
                    .map(|(id, share)| (names.get(&id).cloned().unwrap_or_default(), share))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn edited(&self, field: i32, text: &str) {
        if !(PRICE..=WEIGHED).contains(&field) {
            return;
        }
        self.drafts.borrow_mut()[field as usize] = text.to_owned();
        if field == WEIGHED {
            // What the bottle lost to the print, weighed: the losses are what
            // it lost past what the slice cures.
            let liquid = self.ctrl.last_estimate().and_then(|e| e.liquid_g);
            if let (Some(weighed), Some(liquid)) = (parse_number(text), liquid)
                && weighed > 0.0
                && liquid > 0.0
            {
                let losses = ((weighed / liquid - 1.0) * 100.0).max(0.0);
                let losses = format!("{losses:.1}");
                if let Some(ui) = self.ui.upgrade() {
                    ui.set_slice_price_losses(losses.clone().into());
                }
                self.drafts.borrow_mut()[LOSSES as usize] = losses;
            } else {
                return;
            }
        }
        self.store();
    }

    /// The fields as typed, into the user's settings: a price and a content
    /// above zero make the price; an empty price takes it away.
    fn store(&self) {
        let Some(resin) = self.shown.borrow().clone() else {
            return;
        };
        let drafts = self.drafts.borrow().clone();
        let price = parse_number(&drafts[PRICE as usize]);
        let amount = parse_number(&drafts[AMOUNT as usize]);
        let entry = match (price, amount) {
            (Some(price), Some(amount)) if price > 0.0 && amount > 0.0 => Some(ResinPrice {
                price,
                currency: drafts[CURRENCY as usize].trim().to_owned(),
                amount,
                unit: *self.unit.borrow(),
                losses_pct: parse_number(&drafts[LOSSES as usize])
                    .filter(|v| *v >= 0.0)
                    .unwrap_or(0.0),
            }),
            _ => None,
        };
        // A price still being typed leaves the stored one as it was, until
        // the field is emptied.
        if entry.is_none() && !drafts[PRICE as usize].trim().is_empty() {
            return;
        }
        if self.bus.snapshot().resin_prices.get(&resin) == entry.as_ref() {
            return;
        }
        self.bus.update(move |snap| {
            match entry {
                Some(entry) => {
                    snap.resin_prices.insert(resin, entry);
                }
                None => {
                    snap.resin_prices.remove(&resin);
                }
            }
            PreferencesChange::ResinPrices
        });
    }

    /// What the held slice costs, and each model of it.
    fn show_cost(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let estimate = self.ctrl.last_estimate();
        let price = self
            .shown
            .borrow()
            .as_ref()
            .and_then(|id| self.bus.snapshot().resin_prices.get(id).cloned());
        let cost = estimate.zip(price.as_ref()).and_then(|(e, p)| {
            p.cost_of(e.volume_ml, e.liquid_g)
                .map(|c| (c, p.currency.clone(), p.losses_pct))
        });
        ui.set_slice_cost(
            cost.as_ref()
                .map(|(c, currency, _)| money(currency, *c))
                .unwrap_or_default()
                .into(),
        );
        let note = match (&estimate, &price, &cost) {
            (None, _, _) => SharedString::default(),
            (Some(_), None, _) => ctr!(
                "Inspector",
                "Write what a bottle costs and holds to see what this print costs."
            ),
            (Some(_), Some(_), None) => ctr!(
                "Inspector",
                "This resin gives no density: price its bottle in ml."
            ),
            (Some(_), Some(_), Some((_, _, losses))) if *losses > 0.0 => ctr!(
                "Inspector",
                "The resin it cures, plus {} % lost, at your price.",
                format!("{losses:.1}")
            ),
            (Some(_), Some(_), Some(_)) => {
                ctr!("Inspector", "The resin it cures, at your price.")
            }
        };
        ui.set_slice_cost_note(note);
        let shares = self.shares.borrow();
        let rows: Vec<SharedString> = match estimate {
            Some(e) if shares.len() > 1 => shares
                .iter()
                .map(|(name, share)| {
                    let ml = e.volume_ml * share;
                    match &cost {
                        Some((c, currency, _)) => {
                            format!("{name}: {ml:.1} ml · {}", money(currency, c * share))
                        }
                        None => format!("{name}: {ml:.1} ml"),
                    }
                    .into()
                })
                .collect(),
            _ => Vec::new(),
        };
        let current = ui.get_slice_cost_rows();
        let same = current.row_count() == rows.len()
            && (0..rows.len()).all(|i| current.row_data(i).as_ref() == Some(&rows[i]));
        if !same {
            ui.set_slice_cost_rows(ModelRc::new(VecModel::from(rows)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price(unit: BottleUnit, losses_pct: f64) -> ResinPrice {
        ResinPrice {
            price: 30.0,
            currency: "$".into(),
            amount: 1000.0,
            unit,
            losses_pct,
        }
    }

    #[test]
    fn a_print_costs_its_resin_and_its_losses_at_the_bottle_s_price() {
        // 25 ml of a 1000 ml bottle at 30: 0.75; with 10 % lost, 0.825.
        let cost = price(BottleUnit::Ml, 0.0)
            .cost_of(25.0, Some(27.5))
            .unwrap();
        assert!((cost - 0.75).abs() < 1e-12);
        let cost = price(BottleUnit::Ml, 10.0)
            .cost_of(25.0, Some(27.5))
            .unwrap();
        assert!((cost - 0.825).abs() < 1e-12);
        // By weight: 27.5 g of a 1000 g bottle.
        let cost = price(BottleUnit::G, 0.0).cost_of(25.0, Some(27.5)).unwrap();
        assert!((cost - 0.825).abs() < 1e-12);
        assert_eq!(price(BottleUnit::G, 0.0).cost_of(25.0, None), None);
    }

    #[test]
    fn numbers_take_a_comma_and_money_its_currency() {
        assert_eq!(parse_number(" 3,5 "), Some(3.5));
        assert_eq!(parse_number("x"), None);
        assert_eq!(money("$", 0.826), "$ 0.83");
        assert_eq!(money("", 2.0), "2.00");
    }

    #[test]
    fn the_price_round_trips_and_leaves_its_defaults_out() {
        let p = price(BottleUnit::Ml, 0.0);
        let text = toml::to_string(&p).unwrap();
        assert!(!text.contains("unit") && !text.contains("losses"), "{text}");
        assert_eq!(toml::from_str::<ResinPrice>(&text).unwrap(), p);
        let p = price(BottleUnit::G, 7.5);
        assert_eq!(
            toml::from_str::<ResinPrice>(&toml::to_string(&p).unwrap()).unwrap(),
            p
        );
    }

    #[test]
    fn a_cube_twice_the_size_takes_eight_times_the_share() {
        use sc_mesh_core::transform::AffineTransform3D;
        use sc_scene::Scene;
        use sc_scene::node::LeafPayload;
        let mut graph = Scene::new();
        let mut leaves = Vec::new();
        for (size, x) in [(10.0, -30.0), (20.0, 30.0)] {
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
                        [[size, 0.0, 0.0], [0.0, size, 0.0], [0.0, 0.0, size]],
                        [x, 0.0, size / 2.0],
                    )),
                    LeafPayload::Mesh(std::sync::Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            leaves.push(leaf);
        }
        let spatial = SceneSpatial::from_graph(graph);
        let shares = model_shares(&spatial, &|_| true, &|_| None);
        assert_eq!(shares.len(), 2);
        assert!((shares[0].1 - 1.0 / 9.0).abs() < 1e-6, "{shares:?}");
        assert!((shares[1].1 - 8.0 / 9.0).abs() < 1e-6, "{shares:?}");
    }
}
