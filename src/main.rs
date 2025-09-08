use anyhow::{bail, Context, Result};
use argh::FromArgs;
use clang::{Clang, Index};

use crate::types::LinterSharedState;

mod lints;
mod types;
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
    /// allow applying automatic fixes even when repo has unstaged changes
    #[argh(switch)]
    allow_dirty: bool,
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
        if !args.allow_dirty
            && utils::repo_has_unstaged_or_untracked().context("Failed to get git repo status")?
        {
            bail!("Automatic fixes will not be applied because unstaged changes were found. (Use --allow-dirty to override this)");
        }
        utils::write_changes_to_files(shared_state.fixes)
            .context("Failed to write fixes to source files")?;
    }

    Ok(())
}
