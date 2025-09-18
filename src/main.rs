use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use argh::FromArgs;
use clang::{Clang, CompilationDatabase, Index};

use crate::types::LinterSharedState;

mod lints;
mod types;
mod utils;

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
    /// lint only specified files
    #[argh(positional, greedy)]
    files: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();

    for path in &args.files {
        if !path.is_file() {
            bail!("{path:?}: Not a file");
        }
        if path.extension().unwrap_or_default() != "cpp" {
            bail!("{path:?}: Not a cpp file");
        }
    }

    let clang = Clang::new().map_err(anyhow::Error::msg)?;
    let index = Index::new(&clang, false, false);

    let commands = CompilationDatabase::from_directory("build")
        .ok()
        .context("Failed to parse build/compile_commands.json, please run setup.py first")?;

    let (declarations, definitions, types) =
        utils::get_project_function_and_types(args.all, commands, &index, args.files)
            .context("Failed to get project function and types")?;

    let mut shared_state = LinterSharedState::new(declarations, definitions, types, args.fix);

    lints::lint_functions_and_type_declarations(&mut shared_state);

    if args.fix {
        if !args.allow_dirty
            && utils::repo_has_unstaged_or_untracked().context("Failed to get git repo status")?
        {
            bail!("Automatic fixes will not be applied because unstaged changes were found.\nPlease commit or stage your current progress to ensure nothing is lost.\nAlternatively, if you are really sure, use --allow-dirty to apply fixes anyways.");
        }
        utils::write_changes_to_files(shared_state.fixes)
            .context("Failed to write fixes to source files")?;
    }

    Ok(())
}
