//! Encoding tables for MIPS64.

use super::registers::*;
use crate::ir;
use crate::isa;
use crate::isa::constraints::*;
use crate::isa::enc_tables::*;
use crate::isa::encoding::{base_size, RecipeSizing};
use crate::predicates;

include!(concat!(env!("OUT_DIR"), "/encoding-mips64.rs"));
include!(concat!(env!("OUT_DIR"), "/legalize-mips64.rs"));
