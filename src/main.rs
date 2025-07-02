use std::collections::HashSet;

use anyhow::{Context, Result};
use argh::FromArgs;
use ast_types::TypeDeclaration;
use clang::{Clang, Index};
use utils::{FilePathToChangesMap, SymbolToFunctionInfoMap};

mod ast_types;
mod lints;
mod utils;

const INCLUDE_DIRS: [&str; 6] = [
    "src",
    "lib/al",
    "lib/sead",
    "lib/NintendoSDK",
    "lib/agl",
    "lib/eui",
];

/// lint decomp project using libclang
#[derive(FromArgs)]
struct Args {
    /// automatically apply fixes for all checks
    #[argh(switch)]
    fix: bool,
    /// check all library directories and not just `src` and `lib/al`
    #[argh(switch, short = 'a')]
    all: bool,
}

struct LinterSharedState {
    pub definitions: SymbolToFunctionInfoMap,
    pub declarations: SymbolToFunctionInfoMap,
    pub types: HashSet<TypeDeclaration>,
    pub fixes: FilePathToChangesMap,
    pub auto_fix: bool,
}

impl LinterSharedState {
    fn new(
        declarations: SymbolToFunctionInfoMap,
        definitions: SymbolToFunctionInfoMap,
        types: HashSet<TypeDeclaration>,
        auto_fix: bool,
    ) -> LinterSharedState {
        LinterSharedState {
            definitions,
            declarations,
            types,
            fixes: FilePathToChangesMap::new(),
            auto_fix,
        }
    }
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();
    let clang = Clang::new().map_err(anyhow::Error::msg)?;
    let index = Index::new(&clang, false, false);

    let check_paths: &[&'static str] = if args.all {
        &INCLUDE_DIRS
    } else {
        &["src", "lib/al"]
    };

    let cwd = utils::cwd_string();

    let include_flags = INCLUDE_DIRS.map(|d| format!("-I{cwd}/{d}"));

    let (declarations, definitions, types) =
        utils::get_project_function_and_types(check_paths, &include_flags, &index)
            .context("Failed to get project function and types")?;

    let mut shared_state = LinterSharedState::new(declarations, definitions, types, args.fix);

    lints::lint_functions_and_type_declarations(&mut shared_state);

    if args.fix {
        utils::write_changes_to_files(shared_state.fixes)
            .context("Failed to write fixes to suorce files")?;
    }

    Ok(())
}
