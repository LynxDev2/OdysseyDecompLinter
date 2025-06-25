use crate::ast_types::{FunctionInfo, TypeDeclaration};
use anyhow::Result;
use clang::{EntityVisitResult, Index};
use std::{
    collections::{HashMap, HashSet},
    fs::{read_dir, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub fn find_functions_and_types_in_tu(
    tu_ast: clang::Entity,
) -> (Vec<(String, FunctionInfo)>, Vec<TypeDeclaration>) {
    use clang::EntityKind::*;
    let mut functions: Vec<(String, FunctionInfo)> = Vec::with_capacity(200);
    let mut type_decls: Vec<TypeDeclaration> = Vec::with_capacity(50);
    tu_ast.visit_children(|child: clang::Entity, _| {
        match child.get_kind() {
            FunctionDecl | Method | Constructor => {
                let mangled_name = child.get_mangled_name();
                if let Some(mangled_name) = mangled_name {
                    functions.push((mangled_name, FunctionInfo::new(&child)));
                }
            }
            StructDecl | ClassDecl => {
                // Skip forward-declarations
                if !child.get_children().is_empty() {
                    type_decls.push(TypeDeclaration::new(&child));
                }
            }
            _ => {}
        }
        EntityVisitResult::Recurse
    });
    (functions, type_decls)
}

fn get_cpp_files_recursive(path: impl AsRef<Path>) -> std::io::Result<Vec<PathBuf>> {
    let mut buf = vec![];
    let entries = read_dir(path)?;

    for entry in entries {
        let entry = entry?;
        let meta = entry.metadata()?;

        if meta.is_dir() {
            let mut subdir = get_cpp_files_recursive(entry.path())?;
            buf.append(&mut subdir);
        }

        if meta.is_file() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "cpp") {
                buf.push(entry.path());
            }
        }
    }

    Ok(buf)
}

pub type SymbolToFunctionInfoMap = HashMap<String, FunctionInfo>;

/// Gets all function declarations (map 1), function definitions (map 2) and type declarations
/// (hash set) of the project
pub fn get_project_function_and_types(
    dirs: &[impl AsRef<Path>],
    clang_flags: &[impl AsRef<str>],
    clang_index: &Index,
) -> Result<(
    SymbolToFunctionInfoMap,
    SymbolToFunctionInfoMap,
    HashSet<TypeDeclaration>,
)> {
    let mut decl_map: HashMap<String, FunctionInfo> = HashMap::with_capacity(25_000);
    let mut def_map: HashMap<String, FunctionInfo> = HashMap::with_capacity(10_000);
    let mut type_set: HashSet<TypeDeclaration> = HashSet::with_capacity(500);

    for dir in dirs {
        for file in get_cpp_files_recursive(dir)? {
            let tu = clang_index.parser(&file).arguments(clang_flags).parse()?;
            let (functions, types) = find_functions_and_types_in_tu(tu.get_entity());
            for (mangled_name, function) in functions {
                if function.is_declaration {
                    decl_map.insert(mangled_name, function);
                } else {
                    def_map.insert(mangled_name, function);
                }
            }
            type_set.extend(types);
        }
    }
    Ok((decl_map, def_map, type_set))
}

// Used to store the changes to files that should be applied once all checks have been completed.
// These can't be strings that are directly changed because the file data libclang points to
// wouldn't change causing there to be an index mismatch for the next change
pub type FilePathToChangesMap =
    std::collections::HashMap<String, Vec<(std::ops::Range<usize>, String)>>;

pub fn write_changes_to_files(changes_map: FilePathToChangesMap) -> std::io::Result<()> {
    for (path, changes) in changes_map {
        let mut contents = String::new();
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        file.read_to_string(&mut contents)?;

        let mut replacements = changes;
        // Sort changes by range start in descending order to avoid different sized replacements
        // from shifting the index of other replacements
        replacements.sort_by(|a, b| b.0.start.cmp(&a.0.start));

        for (range, change) in replacements {
            contents.replace_range(range, &change);
        }

        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(contents.as_bytes())?;
        file.flush()?;
    }
    Ok(())
}

pub fn print_lint_fail_for_location(path: &str, line: u32, warning: &str) {
    println!(
        "File {} line {line}: {warning}",
        make_path_relative_from_cwd(path),
    );
}

fn make_path_relative_from_cwd(path: &str) -> String {
    let mut path_owned = path.to_string();
    path_owned = path_owned
        .strip_prefix(&format!("{}/", cwd_string()))
        .unwrap_or(&path_owned)
        .to_string();
    path_owned
}

pub fn change_str_capitalization(input: &str, capitalize: bool) -> String {
    if input.is_empty() {
        return String::new();
    }
    let mut chars: Vec<char> = input.chars().collect();
    if capitalize {
        chars[0] = chars[0].to_ascii_uppercase();
    } else {
        chars[0] = chars[0].to_ascii_lowercase();
    }
    chars.into_iter().collect()
}

pub fn cwd_string() -> String {
    let cwd = std::env::current_dir().expect(
        "Failed to get current work dir, maybe this is being ran from an invalid directory?",
    );
    cwd.to_str()
        .expect("Current work dir should always be valid as a &str")
        .to_string()
}
