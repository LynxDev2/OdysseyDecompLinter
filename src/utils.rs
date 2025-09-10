use crate::types::{FilePathToChangesMap, FunctionInfo, SymbolToFunctionInfoMap, TypeDeclaration};
use anyhow::Result;
use clang::Index;
use colorize::AnsiColor;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, read_dir, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

pub fn find_functions_and_types_in_tu(
    tu_ast: clang::Entity,
) -> (
    SymbolToFunctionInfoMap,
    SymbolToFunctionInfoMap,
    HashSet<TypeDeclaration>,
) {
    let mut decl_map = HashMap::with_capacity(150);
    let mut def_map = HashMap::with_capacity(50);
    let mut type_set = HashSet::with_capacity(100);
    let mut namespace = Vec::new();
    get_functions_and_types_with_namespace(
        tu_ast,
        &mut decl_map,
        &mut def_map,
        &mut type_set,
        &mut namespace,
    );
    (decl_map, def_map, type_set)
}

fn get_functions_and_types_with_namespace(
    entity: clang::Entity,
    decl_map: &mut SymbolToFunctionInfoMap,
    def_map: &mut SymbolToFunctionInfoMap,
    type_set: &mut HashSet<TypeDeclaration>,
    namespace: &mut Vec<String>,
) {
    use clang::EntityKind::*;
    let children = entity.get_children();
    match entity.get_kind() {
        Namespace => {
            namespace.push(entity.get_name().clone().unwrap_or_default());
            for child in children {
                get_functions_and_types_with_namespace(
                    child, decl_map, def_map, type_set, namespace,
                );
            }
            namespace.pop(); // exit namespace
        }
        FunctionDecl | Method | Constructor | Destructor => {
            let label = entity.get_mangled_name().unwrap_or_else(|| {
                entity
                    .get_name()
                    .expect("Function entities should always have a valid name field")
            });
            let function = FunctionInfo::new(&entity, namespace.clone());
            if entity.is_definition() {
                def_map.insert(label, function);
            } else {
                decl_map.insert(label, function);
            }
        }
        StructDecl | ClassDecl => {
            // Skip type forward-declarations
            if children.is_empty() {
                return;
            }
            namespace.push(entity.get_name().clone().unwrap_or_default());
            type_set.insert(TypeDeclaration::new(&entity, namespace.clone()));
            for child in children {
                get_functions_and_types_with_namespace(
                    child, decl_map, def_map, type_set, namespace,
                );
            }
            namespace.pop();
        }
        _ => {
            for child in children {
                get_functions_and_types_with_namespace(
                    child, decl_map, def_map, type_set, namespace,
                );
            }
        }
    }
}

fn get_cpp_files_recursive(path: impl AsRef<Path>) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::with_capacity(1_000);
    let entries = read_dir(path)?;

    for entry in entries {
        let entry = entry?;
        let meta = entry.metadata()?;

        if meta.is_dir() {
            let mut subdir = get_cpp_files_recursive(entry.path())?;
            files.append(&mut subdir);
        }

        if meta.is_file() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "cpp") {
                files.push(path);
            }
        }
    }

    Ok(files)
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
    let mut decl_map: SymbolToFunctionInfoMap = HashMap::with_capacity(25_000);
    let mut def_map: SymbolToFunctionInfoMap = HashMap::with_capacity(10_000);
    let mut type_set: HashSet<TypeDeclaration> = HashSet::with_capacity(500);

    let mut read_time: f32 = 0.0;
    for dir in dirs {
        for file in get_cpp_files_recursive(dir)? {
            let file_modified = file.metadata()?.modified()?;
            let tu = clang_index.parser(&file).arguments(clang_flags).parse()?;
            let cache_file_path = format!(".cache/decomp-linter/{}", make_path_relative_from_cwd(file.as_path().to_str().unwrap()).replace("/", "_"));
            let (decls, defs, types) = if fs::metadata(&cache_file_path).is_ok_and(|m| m.modified().is_ok_and(|cache_modified| cache_modified >= file_modified)) {
                let mut cache_file = File::open(cache_file_path)?;
                let now = std::time::SystemTime::now();
                let read = bincode::decode_from_std_read(&mut cache_file, bincode::config::standard())?;
                read_time += now.elapsed()?.as_secs_f32();
                read
            } else {
                let mut tu_data = find_functions_and_types_in_tu(tu.get_entity());
                let (decls, defs, types) = &mut tu_data;
                decls.retain(|k, _| !decl_map.contains_key(k.as_str()));
                defs.retain(|k, _| !decl_map.contains_key(k.as_str()));
                types.retain(|t| !type_set.contains(t));
                let mut cache_file = File::create(cache_file_path)?;
                bincode::encode_into_std_write(tu_data.clone(), &mut cache_file, bincode::config::standard())?;
                tu_data
            };
            decl_map.extend(decls);
            def_map.extend(defs);
            type_set.extend(types);
        }
    }
    println!("Took to read: {}", read_time);
    Ok((decl_map, def_map, type_set))
}

pub fn write_changes_to_files(changes_map: FilePathToChangesMap) -> std::io::Result<()> {
    for (path, mut changes) in changes_map {
        let mut contents = String::new();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.clone())?;
        file.read_to_string(&mut contents)?;

        // Sort changes by range start in descending order to avoid different sized replacements
        // from shifting the index of other replacements
        changes.sort_by(|a, b| b.0.start.cmp(&a.0.start));
        let replacements_iter = changes
            .iter()
            .enumerate()
            .filter(|(i, (r, _))| {
                let Some((prev_r, _)) = changes.get(i - 1) else {
                    return true;
                };
                if r.start == prev_r.start || r.end > prev_r.start {
                    println!("Warning: Removing overlapping fix for file {}", path);
                    return false;
                }
                true
            })
            .map(|(_, c)| c);

        for (range, replacement) in replacements_iter {
            contents.replace_range(range.clone(), replacement);
        }

        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(contents.as_bytes())?;
        file.flush()?;
    }
    Ok(())
}

fn make_path_relative_from_cwd(path: &str) -> String {
    path.to_string()
        .strip_prefix(&format!("{}/", cwd_string()))
        .unwrap_or(path)
        .to_string()
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

pub fn repo_has_unstaged_or_untracked() -> std::io::Result<bool> {
    // check for unstaged changes
    let diff_status = Command::new("git").args(["diff", "--quiet"]).status()?;
    if !diff_status.success() {
        return Ok(true);
    }

    // check for untracked files
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .output()?;
    Ok(!untracked.stdout.is_empty())
}
