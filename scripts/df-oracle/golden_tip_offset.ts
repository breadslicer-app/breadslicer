// Parity oracle for the contact-tip penetration the slice export applies
// (SC-81): calculateTipOffset with the default material anti-aliasing
// settings (no custom override), over layer heights and pixel pitches of
// real and synthetic printers, square and not, plus the clamped extremes.
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_tip_offset.json
import { DEFAULT_MATERIAL_ANTI_ALIASING_SETTINGS } from './src/features/profiles/profileStore';
import { calculateTipOffset } from './src/supports/rendering/calculateTipOffset';

const layers = [0.0001, 0.01, 0.02, 0.025, 0.03, 0.04, 0.05, 0.06, 0.075, 0.1, 0.15, 0.2];
const pitches: Array<[number, number]> = [
    [0.00001, 0.00001],
    [0.014, 0.014],
    [0.019, 0.019],
    [0.01905, 0.01905],
    [0.022, 0.022],
    [0.024, 0.024],
    [0.028, 0.028],
    [0.035, 0.035],
    [0.05, 0.05],
    [0.0, 0.05],
    [0.05, 0.08],
    [0.1, 0.1],
    [0.143 / 2560 * 1000, 0.089 / 1620 * 1000],
    [0.019, 0.024],
    [0.2, 0.2],
];
const cases = [];
for (const layer of layers) {
    for (const [px, py] of pitches) {
        cases.push({
            layer,
            px,
            py,
            tip: calculateTipOffset(DEFAULT_MATERIAL_ANTI_ALIASING_SETTINGS, layer, px, py),
        });
    }
}
console.log(JSON.stringify({ cases }));
