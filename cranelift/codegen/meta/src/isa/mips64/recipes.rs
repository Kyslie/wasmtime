use std::collections::HashMap;

// use crate::cdsl::instructions::InstructionPredicate;
// use crate::cdsl::recipes::{EncodingRecipeBuilder, EncodingRecipeNumber, Recipes, Stack};
use crate::cdsl::recipes::{EncodingRecipeBuilder, EncodingRecipeNumber, Recipes};
use crate::cdsl::regs::IsaRegs;
use crate::shared::Definitions as SharedDifinitions;

/// An helper to create recipes and use them when defining the MIPS64 encodings.
pub(crate) struct RecipeGroup {
    /// The actuallylist of recipes explicitly created in this file.
    pub recipes: Recipes,

    /// Provides fast lookup from a name to an encoding recipe.
    name_to_recipe: HashMap<String, EncodingRecipeNumber>,
}

impl RecipeGroup {
    fn new() -> Self {
        Self {
            recipes: Recipes::new(),
            name_to_recipe: HashMap::new(),
        }
    }

    fn push(&mut self, builder: EncodingRecipeBuilder) {
        assert!(
            self.name_to_recipe.get(&builder.name).is_none(),
            format!("mips64 recipe '{}' created twice", builder.name)
        );
        let name = builder.name.clone();
        let number = self.recipes.push(builder.build());
        self.name_to_recipe.insert(name, number);
    }

    pub fn by_name(&self, name: &str) -> EncodingRecipeNumber {
        *self
            .name_to_recipe
            .get(name)
            .unwrap_or_else(|| panic!("unknown mips64 recipe name {}", name))
    }

    pub fn collect(self) -> Recipes {
        self.recipes
    }
}

pub(crate) fn define(shared_defs: &SharedDifinitions, regs: &IsaRegs) -> RecipeGroup {
    let format = &shared_defs.formats;

    // Register classes shorthands
    let gpr = regs.class_by_name("GPR");

    // Definitions.
    let mut recipes = RecipeGroup::new();

    // R-type 32-bit instructions: These are mostly binary arithmetic instructions.
    recipes.push(
        EncodingRecipeBuilder::new("R", &format.binary, 4)
            .operands_in(vec![gpr, gpr])
            .operands_out(vec![gpr])
            .emit("put_r(bits, in_reg0, in_reg1, out_reg0, sink);"),
    );

    recipes
}
