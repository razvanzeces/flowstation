//! Data types used for signal processing

use num_complex;

pub type RealSample = f32;
pub use std::f32::consts as sample_consts;

pub type ComplexSample = num_complex::Complex<RealSample>;

pub type SampleCount = i64;
