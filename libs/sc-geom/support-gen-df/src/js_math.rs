//! JavaScript's `Math.sin` and `Math.cos` as V8 computes them.
//!
//! V8 (`src/base/ieee754.cc`) evaluates them with the original fdlibm 5.3
//! (Sun Microsystems, freely distributable: "Permission to use, copy, modify,
//! and distribute this software is freely granted, provided that this notice
//! is preserved"): `__kernel_sin`, `__kernel_cos` with the `qx` correction, and
//! `__ieee754_rem_pio2`. The platform libm and musl's fdlibm (the `libm` crate)
//! use FreeBSD's later kernels, which differ from these in the last bit for ~1%
//! of arguments (checked against Node 26 / V8 14.6 in `tests/js_math.rs`); an
//! ulp of an angle becomes many ulps of a support position downstream.
//!
//! Arguments past the medium reduction range (|x| > 2^19·pi/2 ≈ 823 550) need
//! fdlibm's Payne–Hanek `__kernel_rem_pio2`; they fall back to the `libm`
//! crate, whose reduction is that algorithm. DragonFruit's angles are radians
//! of a support's geometry and never get there.

/// The high word of a double (`__HI`).
#[inline]
fn hi(x: f64) -> i32 {
    (x.to_bits() >> 32) as u32 as i32
}

/// A double with the given high word and a zero low word.
#[inline]
fn from_hi(word: i32) -> f64 {
    f64::from_bits((word as u32 as u64) << 32)
}

const HALF: f64 = 0.5;

// k_sin.c
const S1: f64 = f64::from_bits(0xbfc55555_55555549); // -1.66666666666666324348e-01
const S2: f64 = f64::from_bits(0x3f811111_1110f8a6); // 8.33333333332248946124e-03
const S3: f64 = f64::from_bits(0xbf2a01a0_19c161d5); // -1.98412698298579493134e-04
const S4: f64 = f64::from_bits(0x3ec71de3_57b1fe7d); // 2.75573137070700676789e-06
const S5: f64 = f64::from_bits(0xbe5ae5e6_8a2b9ceb); // -2.50507602534068634195e-08
const S6: f64 = f64::from_bits(0x3de5d93a_5acfd57c); // 1.58969099521155010221e-10

// k_cos.c
const C1: f64 = f64::from_bits(0x3fa55555_5555554c); // 4.16666666666666019037e-02
const C2: f64 = f64::from_bits(0xbf56c16c_16c15177); // -1.38888888888741095749e-03
const C3: f64 = f64::from_bits(0x3efa01a0_19cb1590); // 2.48015872894767294178e-05
const C4: f64 = f64::from_bits(0xbe927e4f_809c52ad); // -2.75573143513906633035e-07
const C5: f64 = f64::from_bits(0x3e21ee9e_bdb4b1c4); // 2.08757232129817482790e-09
const C6: f64 = f64::from_bits(0xbda8fae9_be8838d4); // -1.13596475577881948265e-11

// e_rem_pio2.c
const INVPIO2: f64 = f64::from_bits(0x3fe45f30_6dc9c883); // 6.36619772367581382433e-01
const PIO2_1: f64 = f64::from_bits(0x3ff921fb_54400000); // 1.57079632673412561417e+00
const PIO2_1T: f64 = f64::from_bits(0x3dd0b461_1a626331); // 6.07710050650619224932e-11
const PIO2_2: f64 = f64::from_bits(0x3dd0b461_1a600000); // 6.07710050630396597660e-11
const PIO2_2T: f64 = f64::from_bits(0x3ba3198a_2e037073); // 2.02226624879595063154e-21
const PIO2_3: f64 = f64::from_bits(0x3ba3198a_2e000000); // 2.02226624871116645580e-21
const PIO2_3T: f64 = f64::from_bits(0x397b839a_252049c1); // 8.47842766036889956997e-32

/// High words of n·pi/2, n = 1..32.
const NPIO2_HW: [i32; 32] = [
    0x3FF921FB, 0x400921FB, 0x4012D97C, 0x401921FB, 0x401F6A7A, 0x4022D97C, 0x4025FDBB, 0x402921FB,
    0x402C463A, 0x402F6A7A, 0x4031475C, 0x4032D97C, 0x40346B9C, 0x4035FDBB, 0x40378FDB, 0x403921FB,
    0x403AB41B, 0x403C463A, 0x403DD85A, 0x403F6A7A, 0x40407E4C, 0x4041475C, 0x4042106C, 0x4042D97C,
    0x4043A28C, 0x40446B9C, 0x404534AC, 0x4045FDBB, 0x4046C6CB, 0x40478FDB, 0x404858EB, 0x404921FB,
];

/// `__kernel_sin(x, y, iy)` on [-pi/4, pi/4]; `y` is the tail of `x`, `iy`
/// whether it is used.
fn kernel_sin(x: f64, y: f64, iy: i32) -> f64 {
    let ix = hi(x) & 0x7fffffff; // high word of x
    if ix < 0x3e400000 && (x as i32) == 0 {
        // |x| < 2**-27, generate inexact
        return x;
    }
    let z = x * x;
    let v = z * x;
    let r = S2 + z * (S3 + z * (S4 + z * (S5 + z * S6)));
    if iy == 0 {
        x + v * (S1 + z * r)
    } else {
        x - ((z * (HALF * y - v * r) - y) - v * S1)
    }
}

/// `__kernel_cos(x, y)` on [-pi/4, pi/4].
fn kernel_cos(x: f64, y: f64) -> f64 {
    let ix = hi(x) & 0x7fffffff; // ix = |x|'s high word
    if ix < 0x3e400000 && (x as i32) == 0 {
        // if x < 2**27, generate inexact
        return 1.0;
    }
    let z = x * x;
    let r = z * (C1 + z * (C2 + z * (C3 + z * (C4 + z * (C5 + z * C6)))));
    if ix < 0x3FD33333 {
        // if |x| < 0.3
        return 1.0 - (HALF * z - (z * r - x * y));
    }
    let qx = if ix > 0x3fe90000 {
        // x > 0.78125
        0.28125
    } else {
        from_hi(ix - 0x00200000) // x/4
    };
    let hz = HALF * z - qx;
    let a = 1.0 - qx;
    a - (hz - (z * r - x * y))
}

/// `__ieee754_rem_pio2` for |x| <= 2^19·pi/2: `x = n·pi/2 + y0 + y1`.
/// `None` for larger arguments.
fn rem_pio2(x: f64) -> Option<(i32, f64, f64)> {
    let hx = hi(x);
    let ix = hx & 0x7fffffff;
    if ix <= 0x3fe921fb {
        // |x| ~<= pi/4 , no need for reduction
        return Some((0, x, 0.0));
    }
    if ix < 0x4002d97c {
        // |x| < 3pi/4, special case with n=+-1
        if hx > 0 {
            let mut z = x - PIO2_1;
            let (y0, y1);
            if ix != 0x3ff921fb {
                // 33+53 bit pi is good enough
                y0 = z - PIO2_1T;
                y1 = (z - y0) - PIO2_1T;
            } else {
                // near pi/2, use 33+33+53 bit pi
                z -= PIO2_2;
                y0 = z - PIO2_2T;
                y1 = (z - y0) - PIO2_2T;
            }
            return Some((1, y0, y1));
        }
        // negative x
        let mut z = x + PIO2_1;
        let (y0, y1);
        if ix != 0x3ff921fb {
            y0 = z + PIO2_1T;
            y1 = (z - y0) + PIO2_1T;
        } else {
            z += PIO2_2;
            y0 = z + PIO2_2T;
            y1 = (z - y0) + PIO2_2T;
        }
        return Some((-1, y0, y1));
    }
    if ix > 0x413921fb {
        return None;
    }
    // |x| ~<= 2^19*(pi/2), medium size
    let mut t = x.abs();
    let n = (t * INVPIO2 + HALF) as i32;
    let fnn = f64::from(n);
    let mut r = t - fnn * PIO2_1;
    let mut w = fnn * PIO2_1T; // 1st round good to 85 bit
    let mut y0 = r - w;
    if !(n < 32 && ix != NPIO2_HW[(n - 1) as usize]) {
        // quick check no cancellation failed
        let j = ix >> 20;
        let mut i = j - ((hi(y0) >> 20) & 0x7ff);
        if i > 16 {
            // 2nd iteration needed, good to 118
            t = r;
            w = fnn * PIO2_2;
            r = t - w;
            w = fnn * PIO2_2T - ((t - r) - w);
            y0 = r - w;
            i = j - ((hi(y0) >> 20) & 0x7ff);
            if i > 49 {
                // 3rd iteration need, 151 bits acc
                t = r; // will cover all possible cases
                w = fnn * PIO2_3;
                r = t - w;
                w = fnn * PIO2_3T - ((t - r) - w);
                y0 = r - w;
            }
        }
    }
    let y1 = (r - y0) - w;
    if hx < 0 {
        Some((-n, -y0, -y1))
    } else {
        Some((n, y0, y1))
    }
}

/// `Math.sin`.
pub fn sin(x: f64) -> f64 {
    let ix = hi(x) & 0x7fffffff;
    if ix <= 0x3fe921fb {
        return kernel_sin(x, 0.0, 0);
    }
    if ix >= 0x7ff00000 {
        // sin(Inf or NaN) is NaN (fdlibm's `x - x`)
        return f64::NAN;
    }
    let Some((n, y0, y1)) = rem_pio2(x) else {
        return libm::sin(x);
    };
    match n & 3 {
        0 => kernel_sin(y0, y1, 1),
        1 => kernel_cos(y0, y1),
        2 => -kernel_sin(y0, y1, 1),
        _ => -kernel_cos(y0, y1),
    }
}

/// `Math.cos`.
pub fn cos(x: f64) -> f64 {
    let ix = hi(x) & 0x7fffffff;
    if ix <= 0x3fe921fb {
        return kernel_cos(x, 0.0);
    }
    if ix >= 0x7ff00000 {
        // cos(Inf or NaN) is NaN (fdlibm's `x - x`)
        return f64::NAN;
    }
    let Some((n, y0, y1)) = rem_pio2(x) else {
        return libm::cos(x);
    };
    match n & 3 {
        0 => kernel_cos(y0, y1),
        1 => -kernel_sin(y0, y1, 1),
        2 => -kernel_cos(y0, y1),
        _ => kernel_sin(y0, y1, 1),
    }
}
