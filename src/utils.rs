use crate::types::{FilePathToChangesMap, FunctionInfo, SymbolToFunctionInfoMap, TypeDeclaration};
use anyhow::Result;
use clang::Index;
use colorize::AnsiColor;
use std::{
    collections::{HashMap, HashSet},
    fs::{read_dir, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub fn find_functions_and_types_in_tu(
    tu_ast: clang::Entity,
) -> (Vec<(String, FunctionInfo, bool)>, Vec<TypeDeclaration>) {
    let mut functions = Vec::with_capacity(200);
    let mut type_decls = Vec::with_capacity(50);
    let mut namespace = Vec::new();
    get_functions_and_types_with_namespace(tu_ast, &mut functions, &mut type_decls, &mut namespace);
    (functions, type_decls)
}

fn get_functions_and_types_with_namespace(
    entity: clang::Entity,
    functions: &mut Vec<(String, FunctionInfo, bool)>,
    type_decls: &mut Vec<TypeDeclaration>,
    namespace: &mut Vec<String>,
) {
    use clang::EntityKind::*;
    match entity.get_kind() {
        Namespace => {
            namespace.push(entity.get_name().clone().unwrap_or_default());
            for child in entity.get_children() {
                get_functions_and_types_with_namespace(child, functions, type_decls, namespace);
            }
            namespace.pop(); // exit namespace
        }
        FunctionDecl | Method | Constructor | Destructor => {
            let label = entity.get_mangled_name().unwrap_or_else(|| {
                entity
                    .get_name()
                    .expect("Function entities should always have a valid name field")
            });
            functions.push((
                label,
                FunctionInfo::new(&entity, namespace.clone()),
                !entity.is_definition(),
            ));
        }
        StructDecl | ClassDecl => {
            namespace.push(entity.get_name().clone().unwrap_or_default());
            if !entity.get_children().is_empty() {
                type_decls.push(TypeDeclaration::new(&entity, namespace.clone()));
                for child in entity.get_children() {
                    get_functions_and_types_with_namespace(child, functions, type_decls, namespace);
                }
            }
            namespace.pop();
        }
        _ => {
            for child in entity.get_children() {
                get_functions_and_types_with_namespace(child, functions, type_decls, namespace);
            }
        }
    }
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
            for (mangled_name, function, is_declaration) in functions {
                if is_declaration {
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

pub fn print_no_visibility_fix_warning() {
    println!(
        "{} {}",
        "Warning:".bold().red(),
        "Visibility issues can't be automatically fixed, please fix them manually".red()
    );
}

pub fn print_lint_fail_for_location(path: &str, line: u32, warning: &str) {
    println!(
        "{}",
        &format!("{}:{line}: {warning}", make_path_relative_from_cwd(path)).b_yellow()
    );
}

pub fn print_possible_compiler_error_warning_for_line(path: &str, line: u32) {
    println!(
        "{} {}",
        "Warning:".bold().b_red(),
        &format!(
            "{}:{line}: Field name changed, this may cause errors when compiling",
            make_path_relative_from_cwd(path)
        )
        .b_red()
    );
}

pub fn print_fix_success() {
    println!("{}", "Fixed".b_green());
}
