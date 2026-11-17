//! Platform-independent transcendental functions for the generation path.
//!
//! `f64::sin`, `exp`, `powf`, `hypot`… call the platform's maths library (MSVC CRT on
//! Windows, glibc on Linux), whose last bits differ between systems and versions. World and
//! chunk generation goes through [`DetMath`] instead: the pure-Rust `libm` port of musl gives
//! the same bits everywhere (invariant 3, audit M7 E5). `sqrt` is exact (IEEE 754) and needs
//! no wrapper; `powi` is lowered to multiplications by the compiler.
//!
//! The `std-math` feature switches back to the platform functions, only to measure the cost.

/// Deterministic maths (`det_` prefix: the `std` methods of the same name would win).
pub trait DetMath: Copy {
    fn det_sin(self) -> Self;
    fn det_cos(self) -> Self;
    fn det_sin_cos(self) -> (Self, Self);
    fn det_tan(self) -> Self;
    fn det_asin(self) -> Self;
    fn det_acos(self) -> Self;
    fn det_atan(self) -> Self;
    fn det_atan2(self, x: Self) -> Self;
    fn det_tanh(self) -> Self;
    fn det_exp(self) -> Self;
    fn det_exp_m1(self) -> Self;
    fn det_ln(self) -> Self;
    fn det_ln_1p(self) -> Self;
    fn det_log10(self) -> Self;
    fn det_powf(self, e: Self) -> Self;
    fn det_hypot(self, o: Self) -> Self;
}

macro_rules! det_impl {
    ($t:ty, $sin:ident, $cos:ident, $tan:ident, $asin:ident, $acos:ident, $atan:ident,
     $atan2:ident, $tanh:ident, $exp:ident, $expm1:ident, $ln:ident, $ln1p:ident,
     $log10:ident, $pow:ident, $hypot:ident) => {
        #[cfg(not(feature = "std-math"))]
        impl DetMath for $t {
            #[inline]
            fn det_sin(self) -> Self {
                libm::$sin(self)
            }
            #[inline]
            fn det_cos(self) -> Self {
                libm::$cos(self)
            }
            #[inline]
            fn det_sin_cos(self) -> (Self, Self) {
                (libm::$sin(self), libm::$cos(self))
            }
            #[inline]
            fn det_tan(self) -> Self {
                libm::$tan(self)
            }
            #[inline]
            fn det_asin(self) -> Self {
                libm::$asin(self)
            }
            #[inline]
            fn det_acos(self) -> Self {
                libm::$acos(self)
            }
            #[inline]
            fn det_atan(self) -> Self {
                libm::$atan(self)
            }
            #[inline]
            fn det_atan2(self, x: Self) -> Self {
                libm::$atan2(self, x)
            }
            #[inline]
            fn det_tanh(self) -> Self {
                libm::$tanh(self)
            }
            #[inline]
            fn det_exp(self) -> Self {
                libm::$exp(self)
            }
            #[inline]
            fn det_exp_m1(self) -> Self {
                libm::$expm1(self)
            }
            #[inline]
            fn det_ln(self) -> Self {
                libm::$ln(self)
            }
            #[inline]
            fn det_ln_1p(self) -> Self {
                libm::$ln1p(self)
            }
            #[inline]
            fn det_log10(self) -> Self {
                libm::$log10(self)
            }
            #[inline]
            fn det_powf(self, e: Self) -> Self {
                libm::$pow(self, e)
            }
            #[inline]
            fn det_hypot(self, o: Self) -> Self {
                libm::$hypot(self, o)
            }
        }

        #[cfg(feature = "std-math")]
        #[allow(clippy::disallowed_methods)]
        impl DetMath for $t {
            #[inline]
            fn det_sin(self) -> Self {
                self.sin()
            }
            #[inline]
            fn det_cos(self) -> Self {
                self.cos()
            }
            #[inline]
            fn det_sin_cos(self) -> (Self, Self) {
                self.sin_cos()
            }
            #[inline]
            fn det_tan(self) -> Self {
                self.tan()
            }
            #[inline]
            fn det_asin(self) -> Self {
                self.asin()
            }
            #[inline]
            fn det_acos(self) -> Self {
                self.acos()
            }
            #[inline]
            fn det_atan(self) -> Self {
                self.atan()
            }
            #[inline]
            fn det_atan2(self, x: Self) -> Self {
                self.atan2(x)
            }
            #[inline]
            fn det_tanh(self) -> Self {
                self.tanh()
            }
            #[inline]
            fn det_exp(self) -> Self {
                self.exp()
            }
            #[inline]
            fn det_exp_m1(self) -> Self {
                self.exp_m1()
            }
            #[inline]
            fn det_ln(self) -> Self {
                self.ln()
            }
            #[inline]
            fn det_ln_1p(self) -> Self {
                self.ln_1p()
            }
            #[inline]
            fn det_log10(self) -> Self {
                self.log10()
            }
            #[inline]
            fn det_powf(self, e: Self) -> Self {
                self.powf(e)
            }
            #[inline]
            fn det_hypot(self, o: Self) -> Self {
                self.hypot(o)
            }
        }
    };
}

det_impl!(
    f64, sin, cos, tan, asin, acos, atan, atan2, tanh, exp, expm1, log, log1p, log10, pow, hypot
);
det_impl!(
    f32, sinf, cosf, tanf, asinf, acosf, atanf, atan2f, tanhf, expf, expm1f, logf, log1pf, log10f,
    powf, hypotf
);

#[cfg(test)]
mod tests {
    use super::DetMath;

    /// Bit patterns of the musl port: the same on every platform (the CI checks them on
    /// Linux and Windows).
    #[cfg(not(feature = "std-math"))]
    #[test]
    fn libm_bits_are_fixed() {
        let x = 1.234_567_890_123_f64;
        let got = [
            x.det_sin(),
            x.det_cos(),
            x.det_exp(),
            x.det_ln(),
            x.det_powf(0.4),
            x.det_atan2(0.7),
            x.det_hypot(3.1),
            (x * 1e5).det_sin(),
        ]
        .map(f64::to_bits);
        let want = GOLDEN;
        assert_eq!(got, want, "libm results changed: {got:#x?}");
    }

    #[cfg(not(feature = "std-math"))]
    const GOLDEN: [u64; 8] = [
        0x3fee_354b_7e89_c6fd,
        0x3fd5_1d8e_d12b_80cc,
        0x400b_7ec1_cd28_dd0a,
        0x3fca_f8e8_0db6_3fe2,
        0x3ff1_6836_858d_0491,
        0x3ff0_e142_7a49_4401,
        0x400a_b1be_32f6_0465,
        0xbfef_f50d_0b68_a1d3,
    ];

    #[test]
    fn close_to_std() {
        #[allow(clippy::disallowed_methods)]
        for i in 0..1000 {
            let x = f64::from(i) * 0.137 - 50.0;
            assert!((x.det_sin() - x.sin()).abs() < 1e-15);
            assert!((x.det_cos() - x.cos()).abs() < 1e-15);
            let e = (x / 10.0).det_exp();
            assert!((e - (x / 10.0).exp()).abs() <= e * 1e-15);
        }
    }
}
