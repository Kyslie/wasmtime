use crate::cdsl::cpu_modes::CpuMode;
use crate::cdsl::instructions::{InstructionGroupBuilder, InstructionPredicateMap};
use crate::cdsl::isa::TargetIsa;
use crate::cdsl::regs::{IsaRegs, IsaRegsBuilder, RegBankBuilder, RegClassBuilder};
use crate::cdsl::settings::{SettingGroup, SettingGroupBuilder};

use crate::shared::types::Float::{F32, F64};
use crate::shared::types::Int::{I32, I64};
use crate::shared::Definitions as SharedDefinitions;

mod recipes;

fn define_settings(_shared: &SettingGroup) -> SettingGroup {
    let setting = SettingGroupBuilder::new("mips64");
    setting.build()
}
fn define_registers() -> IsaRegs {
    let mut regs = IsaRegsBuilder::new();

    let builder = RegBankBuilder::new("IntRegs", "r")
        .units(32)
        .track_pressure(true);
    let int_regs = regs.add_bank(builder);

    let builder = RegBankBuilder::new("FloatRegs", "f")
        .units(32)
        .track_pressure(true);
    let float_regs = regs.add_bank(builder);

    let builder = RegClassBuilder::new_toplevel("GPR", int_regs);
    regs.add_class(builder);

    let builder = RegClassBuilder::new_toplevel("FPR", float_regs);
    regs.add_class(builder);

    regs.build()
}

pub(crate) fn define(shared_defs: &mut SharedDefinitions) -> TargetIsa {
    let settings = define_settings(&shared_defs.settings);
    let regs = define_registers();

    let inst_group = InstructionGroupBuilder::new(&mut shared_defs.all_instructions).build();

    let mut mips_64 = CpuMode::new("MIPS64");

    let expand = shared_defs.transform_groups.by_name("expand");
    let narrow_flags = shared_defs.transform_groups.by_name("narrow_flags");

    mips_64.legalize_monomorphic(expand);
    mips_64.legalize_default(narrow_flags);
    mips_64.legalize_type(I32, expand);
    mips_64.legalize_type(I64, expand);
    mips_64.legalize_type(F32, expand);
    mips_64.legalize_type(F64, expand);

    let recipes = recipes::define(shared_defs, &regs);

    // let encodings  = encodings::define(shared_defs, &settings, &recipes);
    let encodings_predicates = InstructionPredicateMap::new();

    let recipes = recipes.collect();

    let cpu_modes = vec![mips_64];

    TargetIsa::new(
        "mips64",
        inst_group,
        settings,
        regs,
        recipes,
        cpu_modes,
        encodings_predicates,
    )
    // Todo
}
