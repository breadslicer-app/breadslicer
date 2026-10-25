//! JavaScript's `Math.sin`, `Math.cos`, `Math.log` and `Math.cbrt` as V8
//! computes them. (`Math.pow` is `three::math_utils::js_pow`.)
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

// ---------------------------------------------------------------------------
// Math.log: fdlibm 5.3 `__ieee754_log`, with its `i | j` choice of formula.
// The `libm` crate carries musl's rewrite of it, one combined expression,
// which differs in the last bit for ~0.05% of arguments.

const LN2_HI: f64 = f64::from_bits(0x3fe62e42_fee00000); // 6.93147180369123816490e-01
const LN2_LO: f64 = f64::from_bits(0x3dea39ef_35793c76); // 1.90821492927058770002e-10
const TWO54: f64 = f64::from_bits(0x43500000_00000000); // 1.80143985094819840000e+16
const LG1: f64 = f64::from_bits(0x3fe55555_55555593); // 6.666666666666735130e-01
const LG2: f64 = f64::from_bits(0x3fd99999_9997fa04); // 3.999999999940941908e-01
const LG3: f64 = f64::from_bits(0x3fd24924_94229359); // 2.857142874366239149e-01
const LG4: f64 = f64::from_bits(0x3fcc71c5_1d8e78af); // 2.222219843214978396e-01
const LG5: f64 = f64::from_bits(0x3fc74664_96cb03de); // 1.818357216161805012e-01
const LG6: f64 = f64::from_bits(0x3fc39a09_d078c69f); // 1.531383769920937332e-01
const LG7: f64 = f64::from_bits(0x3fc2f112_df3e5244); // 1.479819860511658591e-01

/// The low word of a double (`__LO`).
#[inline]
fn lo(x: f64) -> u32 {
    x.to_bits() as u32
}

/// `x` with its high word replaced (`SET_HIGH_WORD`).
#[inline]
fn with_hi(x: f64, word: i32) -> f64 {
    f64::from_bits(((word as u32 as u64) << 32) | u64::from(lo(x)))
}

/// `Math.log`.
pub fn log(x: f64) -> f64 {
    let mut x = x;
    let mut hx = hi(x);
    let lx = lo(x);
    let mut k: i32 = 0;
    if hx < 0x00100000 {
        // x < 2**-1022
        if ((hx & 0x7fffffff) as u32 | lx) == 0 {
            return f64::NEG_INFINITY; // log(+-0) = -inf
        }
        if hx < 0 {
            return f64::NAN; // log(-#) = NaN
        }
        k -= 54;
        x *= TWO54; // subnormal number, scale up x
        hx = hi(x);
    }
    if hx >= 0x7ff00000 {
        return x + x;
    }
    k += (hx >> 20) - 1023;
    hx &= 0x000fffff;
    let i = (hx + 0x95f64) & 0x100000;
    x = with_hi(x, hx | (i ^ 0x3ff00000)); // normalize x or x/2
    k += i >> 20;
    let f = x - 1.0;
    if (0x000fffff & (2 + hx)) < 3 {
        // -2**-20 <= f < 2**-20
        if f == 0.0 {
            if k == 0 {
                return 0.0;
            }
            let dk = f64::from(k);
            return dk * LN2_HI + dk * LN2_LO;
        }
        // fdlibm's `0.33333333333333333` literal, as the double it parses to.
        let r = f * f * (0.5 - f64::from_bits(0x3fd55555_55555555) * f);
        if k == 0 {
            return f - r;
        }
        let dk = f64::from(k);
        return dk * LN2_HI - ((r - dk * LN2_LO) - f);
    }
    let s = f / (2.0 + f);
    let dk = f64::from(k);
    let z = s * s;
    let mut i = hx - 0x6147a;
    let w = z * z;
    let j = 0x6b851 - hx;
    let t1 = w * (LG2 + w * (LG4 + w * LG6));
    let t2 = z * (LG1 + w * (LG3 + w * (LG5 + w * LG7)));
    i |= j;
    let r = t2 + t1;
    if i > 0 {
        let hfsq = 0.5 * f * f;
        if k == 0 {
            f - (hfsq - s * (hfsq + r))
        } else {
            dk * LN2_HI - ((hfsq - (s * (hfsq + r) + dk * LN2_LO)) - f)
        }
    } else if k == 0 {
        f - s * (f - r)
    } else {
        dk * LN2_HI - ((s * (f - r) - dk * LN2_LO) - f)
    }
}

// ---------------------------------------------------------------------------
// Math.cbrt: FreeBSD's `s_cbrt.c` (Bruce D. Evans' revision of fdlibm's): a
// bit-pattern seed, a polynomial to 23 bits, one Newton step. The `libm` crate
// now carries core-math's correctly rounded cbrt, which V8 14.6 does not use.

const B1: u32 = 715094163; // (1023-1023/3-0.03306235651)*2**20
const B2: u32 = 696219795; // (1023-1023/3-54/3-0.03306235651)*2**20
const P0: f64 = f64::from_bits(0x3ffe03e6_0f61e692); // 1.87595182427177009643
const P1: f64 = f64::from_bits(0xbffe28e0_92f02420); // -1.88497979543377169875
const P2: f64 = f64::from_bits(0x3ff9f160_4a49d6c2); // 1.621429720105354466140
const P3: f64 = f64::from_bits(0xbfe844cb_bee751d9); // -0.758397934778766047437
const P4: f64 = f64::from_bits(0x3fc2b000_d4e4edd7); // 0.145996192886612446982

/// `Math.cbrt`.
pub fn cbrt(x: f64) -> f64 {
    let bits = x.to_bits();
    let mut hx = (bits >> 32) as u32;
    let low = bits as u32;
    let sign = hx & 0x80000000;
    hx ^= sign;
    if hx >= 0x7ff00000 {
        return x + x; // cbrt(NaN, INF) is itself
    }
    let mut t;
    if hx < 0x00100000 {
        // zero or subnormal
        if (hx | low) == 0 {
            return x; // cbrt(0) is itself
        }
        t = TWO54 * x;
        let high = (t.to_bits() >> 32) as u32;
        t = f64::from_bits(u64::from(sign | ((high & 0x7fffffff) / 3 + B2)) << 32);
    } else {
        t = f64::from_bits(u64::from(sign | (hx / 3 + B1)) << 32);
    }

    // New cbrt to 23 bits: cbrt(x) = t*cbrt(x/t**3) ~= t*P(t**3/x).
    let r = (t * t) * (t / x);
    t *= (P0 + r * (P1 + r * P2)) + ((r * r) * r) * (P3 + r * P4);

    // Round t away from zero to 23 bits.
    t = f64::from_bits((t.to_bits().wrapping_add(0x80000000)) & 0xffffffff_c0000000);

    // One step Newton iteration to 53 bits with error < 0.667 ulps.
    let s = t * t; // t*t is exact
    let mut r = x / s; // error <= 0.5 ulps; |r| < |t|
    let w = t + t; // t+t is exact
    r = (r - t) / (w + r); // r-t is exact; w+r ~= 3*t
    t + t * r // error <= 0.5 + 0.5/3 + epsilon
}
